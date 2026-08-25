//! AI meeting-notes generation.
//!
//! After a meeting session is saved to history, the transcript is sent — in
//! the background — to the user's configured post-processing provider with
//! the meeting-notes prompt (`settings.meeting_notes_prompt`), and the result
//! is stored on the entry as `ai_notes`. Progress is reported to the UI via
//! the untyped `notes-status` event (`generating` / `done` / `failed`), and
//! the stored result additionally arrives through the typed history `Updated`
//! event.

use crate::actions::{strip_invisible_chars, strip_think_block};
use crate::managers::history::HistoryManager;
use crate::settings::{get_settings, APPLE_INTELLIGENCE_PROVIDER_ID};
use log::{debug, error, info};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, serde::Serialize)]
struct NotesStatusEvent {
    id: i64,
    status: &'static str,
}

fn emit_status(app: &AppHandle, id: i64, status: &'static str) {
    let _ = app.emit("notes-status", NotesStatusEvent { id, status });
}

/// Fire-and-forget note generation for a history entry. Errors are logged
/// and surfaced to the UI as a `failed` status; they never block the caller.
pub fn spawn_generation(app: &AppHandle, entry_id: i64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = generate_and_store(&app, entry_id).await {
            error!("Meeting note generation failed for entry {entry_id}: {err}");
            emit_status(&app, entry_id, "failed");
        }
    });
}

async fn generate_and_store(app: &AppHandle, entry_id: i64) -> Result<(), String> {
    let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());
    let entry = hm
        .get_entry_by_id(entry_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("history entry {entry_id} not found"))?;

    let transcript = entry.transcription_text.trim().to_string();
    if transcript.is_empty() {
        return Err("transcript is empty".to_string());
    }

    let settings = get_settings(app);
    let provider = settings
        .active_post_process_provider()
        .cloned()
        .ok_or_else(|| "no AI provider configured (see Post Process settings)".to_string())?;
    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    if model.trim().is_empty() && provider.id != APPLE_INTELLIGENCE_PROVIDER_ID {
        return Err(format!(
            "no model configured for provider '{}' (see Post Process settings)",
            provider.id
        ));
    }

    let mut prompt = settings.meeting_notes_prompt.clone();
    if prompt.trim().is_empty() {
        prompt = crate::settings::default_meeting_notes_prompt();
    }

    emit_status(app, entry_id, "generating");
    info!(
        "Generating meeting notes for entry {} via provider '{}' (model: {}, transcript: {} chars)",
        entry_id,
        provider.id,
        model,
        transcript.len()
    );

    let raw = if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            if !crate::apple_intelligence::check_apple_intelligence_availability() {
                return Err("Apple Intelligence is not available on this device".to_string());
            }
            let token_limit = model.trim().parse::<i32>().unwrap_or(0);
            crate::apple_intelligence::process_text_with_system_prompt(
                &prompt,
                &transcript,
                token_limit,
            )?
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            return Err("Apple Intelligence is not available on this platform".to_string());
        }
    } else {
        let api_key = settings
            .post_process_api_keys
            .get(&provider.id)
            .cloned()
            .unwrap_or_default();
        // Same reasoning opt-out as transcript post-processing: notes don't
        // benefit enough to justify seconds of extra latency on local models.
        let disable_reasoning = matches!(provider.id.as_str(), "custom" | "openrouter");
        crate::llm_client::send_chat_completion_with_schema(
            &provider,
            api_key,
            &model,
            transcript,
            Some(prompt),
            None,
            disable_reasoning,
        )
        .await?
        .ok_or_else(|| "AI response had no content".to_string())?
    };

    let notes = strip_invisible_chars(strip_think_block(&raw));
    let notes = notes.trim();
    if notes.is_empty() {
        return Err("AI returned empty notes".to_string());
    }

    hm.set_ai_notes(entry_id, Some(notes.to_string()))
        .map_err(|e| e.to_string())?;
    emit_status(app, entry_id, "done");
    debug!(
        "Stored meeting notes for entry {} ({} chars)",
        entry_id,
        notes.len()
    );
    Ok(())
}
