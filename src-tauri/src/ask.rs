//! "Ask your notes" answering.
//!
//! An ask session (see `managers::history`) holds one query; this module
//! writes its answer: the matching meetings are gathered as context and sent
//! to the provider the user picked in the search UI (any configured
//! post-processing provider — cloud or local). Progress is reported via the
//! untyped `ask-answer` event (`thinking` / `done` / `failed`), and the
//! stored answer also arrives through `ask-sessions-updated`.

use crate::managers::history::HistoryManager;
use crate::settings::{get_settings, APPLE_INTELLIGENCE_PROVIDER_ID};
use log::{debug, error, info};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

/// Sessions currently being answered — a second request is a no-op.
static IN_FLIGHT: Lazy<Mutex<HashSet<i64>>> = Lazy::new(|| Mutex::new(HashSet::new()));

const MAX_CONTEXT_MEETINGS: usize = 6;
const MAX_CHARS_PER_MEETING: usize = 6000;
const MAX_TOTAL_CONTEXT_CHARS: usize = 24000;

#[derive(Clone, Serialize)]
struct AskAnswerEvent {
    id: i64,
    status: &'static str, // "thinking" | "done" | "failed"
}

fn emit_status(app: &AppHandle, id: i64, status: &'static str) {
    let _ = app.emit("ask-answer", AskAnswerEvent { id, status });
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

/// Answer a session in the background with the given provider (falls back to
/// the active post-processing provider).
pub fn spawn_answer(app: &AppHandle, session_id: i64, provider_id: Option<String>) {
    if !IN_FLIGHT.lock().unwrap().insert(session_id) {
        debug!("Ask session {session_id} is already being answered");
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        emit_status(&app, session_id, "thinking");
        let result = answer(&app, session_id, provider_id).await;
        IN_FLIGHT.lock().unwrap().remove(&session_id);
        match result {
            Ok(()) => emit_status(&app, session_id, "done"),
            Err(err) => {
                error!("Ask session {session_id} failed: {err}");
                emit_status(&app, session_id, "failed");
            }
        }
    });
}

async fn answer(
    app: &AppHandle,
    session_id: i64,
    provider_id: Option<String>,
) -> Result<(), String> {
    let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());
    let session = hm
        .get_ask_session(session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("ask session {session_id} not found"))?;

    let settings = get_settings(app);
    let provider = match provider_id.as_deref() {
        Some(id) => settings
            .post_process_providers
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| format!("provider '{id}' not configured"))?,
        None => settings
            .active_post_process_provider()
            .cloned()
            .ok_or("no provider configured")?,
    };
    if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
        return Err("Apple Intelligence is not supported for ask sessions yet".to_string());
    }
    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    if model.trim().is_empty() {
        return Err(format!(
            "no model configured for provider '{}'",
            provider.id
        ));
    }
    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    let disable_reasoning = matches!(provider.id.as_str(), "custom" | "openrouter");

    // Context: the meetings that match the query, notes preferred over the
    // raw transcript.
    let hits = hm
        .search_meeting_notes(&session.query, MAX_CONTEXT_MEETINGS)
        .map_err(|e| e.to_string())?;
    if hits.is_empty() {
        return Err("no matching meetings".to_string());
    }
    let mut context = String::new();
    for hit in &hits {
        let Ok(Some(entry)) = hm.get_entry_by_id_sync(hit.entry_id) else {
            continue;
        };
        let body = match entry.user_notes.as_deref() {
            Some(text) if !text.trim().is_empty() => text.to_string(),
            _ => entry.ai_notes.clone().unwrap_or_default(),
        };
        let content = if body.trim().is_empty() {
            entry.transcription_text.clone()
        } else {
            format!(
                "{body}\n\nTranscript excerpt:\n{}",
                entry.transcription_text
            )
        };
        let date = chrono::DateTime::from_timestamp(entry.timestamp, 0)
            .map(|utc| {
                utc.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        context.push_str(&format!(
            "## {} ({date})\n{}\n\n",
            entry.title,
            truncate_chars(&content, MAX_CHARS_PER_MEETING)
        ));
        if context.chars().count() > MAX_TOTAL_CONTEXT_CHARS {
            break;
        }
    }

    let system = "You answer questions about the user's own meeting notes. Use ONLY the \
                  meetings provided below the question — never invent facts. Answer \
                  concisely in Markdown, in the same language as the question, and say \
                  which meeting (title and date) the information comes from. If the \
                  meetings don't contain the answer, say so plainly."
        .to_string();
    let user_content = format!("Question: {}\n\nMeetings:\n\n{context}", session.query);

    info!(
        "Answering ask session {session_id} via provider '{}' (model: {model}, {} meetings)",
        provider.id,
        hits.len()
    );
    let raw = crate::llm_client::send_chat_completion_with_schema(
        &provider,
        api_key,
        &model,
        user_content,
        Some(system),
        None,
        disable_reasoning,
    )
    .await?
    .ok_or("empty answer")?;

    let answer = crate::actions::strip_invisible_chars(crate::actions::strip_think_block(&raw));
    let answer = answer.trim();
    if answer.is_empty() {
        return Err("empty answer".to_string());
    }
    hm.set_ask_answer(session_id, Some(answer), Some(&provider.id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn answer_ask_session(
    app: AppHandle,
    id: i64,
    provider_id: Option<String>,
) -> Result<(), String> {
    spawn_answer(&app, id, provider_id);
    Ok(())
}
