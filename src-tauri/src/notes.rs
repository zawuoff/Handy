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
use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

/// Entries whose note generation is currently running. Guards against a
/// second concurrent generation for the same entry (e.g. the user pressing
/// "enhance" while the automatic pass is still waiting on a cold model) and
/// lets the UI recover the "writing" state after (re)mounting.
static IN_FLIGHT: Lazy<Mutex<HashSet<i64>>> = Lazy::new(|| Mutex::new(HashSet::new()));

#[tauri::command]
#[specta::specta]
pub fn get_generating_note_ids() -> Result<Vec<i64>, String> {
    Ok(IN_FLIGHT.lock().unwrap().iter().copied().collect())
}

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
    if !IN_FLIGHT.lock().unwrap().insert(entry_id) {
        debug!("Note generation for entry {entry_id} is already running; not starting another");
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = generate_and_store(&app, entry_id).await;
        IN_FLIGHT.lock().unwrap().remove(&entry_id);
        if let Err(err) = result {
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

    // Second pass: turn the action items into calendar events and todos.
    // Best-effort — a failure here never invalidates the stored notes.
    if let Err(err) = organize_action_items(app, entry_id, notes).await {
        debug!("Action-item extraction skipped for entry {entry_id}: {err}");
    }
    Ok(())
}

/// Create a calendar event through the user's `cal-add` script (GNOME
/// calendar via Evolution Data Server). `when` is natural language — the
/// script hands it to GNU date.
pub(crate) fn create_calendar_event(
    title: &str,
    when: &str,
    duration_min: u32,
) -> Result<(), String> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin/cal-add"));
    }
    candidates.push(std::path::PathBuf::from("cal-add"));

    for cmd in candidates {
        match std::process::Command::new(&cmd)
            .arg(title)
            .arg(when)
            .arg(duration_min.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(status) if status.success() => {
                info!("Calendar event created: '{title}' at '{when}' ({duration_min} min)");
                return Ok(());
            }
            Ok(status) => return Err(format!("cal-add failed for '{when}': {status}")),
            Err(_) => continue, // not at this path — try the next candidate
        }
    }
    Err("cal-add command not found (expected at ~/.local/bin/cal-add)".to_string())
}

#[derive(serde::Deserialize)]
struct ActionItem {
    title: String,
    #[serde(default)]
    when: Option<String>,
    #[serde(default)]
    duration_min: Option<u32>,
}

#[derive(Clone, serde::Serialize)]
struct AutoOrganizedEvent {
    events: u32,
    todos: u32,
}

/// Extract action items from freshly generated notes with a second model
/// call; dated ones become calendar events, undated ones become todos.
fn normalized_title(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

async fn organize_action_items(app: &AppHandle, entry_id: i64, notes: &str) -> Result<(), String> {
    // Atomic one-time claim: regenerations and concurrent generations must
    // never re-create the same events and todos.
    {
        let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());
        if !hm
            .try_claim_action_items(entry_id)
            .map_err(|e| e.to_string())?
        {
            return Err("already organized".to_string());
        }
    }
    let settings = get_settings(app);
    let provider = settings
        .active_post_process_provider()
        .cloned()
        .ok_or("no provider configured")?;
    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    if model.trim().is_empty() {
        return Err("no model configured".to_string());
    }
    if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
        return Err("action-item extraction is not wired to Apple Intelligence yet".to_string());
    }
    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    let disable_reasoning = matches!(provider.id.as_str(), "custom" | "openrouter");

    let today = chrono::Local::now()
        .format("%A, %Y-%m-%d %H:%M")
        .to_string();
    let shape = r#"[{"title": "short task description", "when": "YYYY-MM-DD HH:MM" or null, "duration_min": number or null}]"#;
    let prompt = format!(
        "Extract the action items (tasks, commitments, follow-ups) from the meeting notes. \
         Today is {today}.\n\
         Respond with ONLY a JSON array, no prose and no markdown fence, shaped exactly like:\n\
         {shape}\n\
         Rules: resolve relative dates (tomorrow, Friday, next week) against today's date; \
         if only a day is known use 09:00 as the time; use null for \"when\" if the item has \
         no clear date or deadline; keep titles under 10 words; each real-world action \
         must appear exactly once — merge duplicates and near-duplicates; return [] if \
         there are none."
    );

    let raw = crate::llm_client::send_chat_completion_with_schema(
        &provider,
        api_key,
        &model,
        notes.to_string(),
        Some(prompt),
        None,
        disable_reasoning,
    )
    .await?
    .ok_or("empty extraction response")?;

    // Tolerant parse: take the outermost JSON array in the response.
    let raw = strip_think_block(&raw);
    let start = raw.find('[').ok_or("no JSON array in response")?;
    let end = raw.rfind(']').ok_or("no JSON array in response")?;
    let items: Vec<ActionItem> =
        serde_json::from_str(&raw[start..=end]).map_err(|e| format!("bad JSON: {e}"))?;

    let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());
    // Skip items already covered: seen in this run, or an existing open todo.
    let mut seen: std::collections::HashSet<String> = hm
        .get_todos()
        .map(|existing| {
            existing
                .iter()
                .filter(|todo| !todo.done)
                .map(|todo| normalized_title(&todo.title))
                .collect()
        })
        .unwrap_or_default();
    let mut events = 0u32;
    let mut todos = 0u32;
    for item in items.into_iter().take(10) {
        let title = item.title.trim().to_string();
        if title.is_empty() {
            continue;
        }
        if !seen.insert(normalized_title(&title)) {
            debug!("Skipping duplicate action item: '{title}'");
            continue;
        }
        let scheduled = match item.when.as_deref().map(str::trim) {
            Some(when) if !when.is_empty() => {
                match create_calendar_event(&title, when, item.duration_min.unwrap_or(60)) {
                    Ok(()) => {
                        events += 1;
                        true
                    }
                    Err(err) => {
                        debug!("Falling back to todo for '{title}': {err}");
                        false
                    }
                }
            }
            _ => false,
        };
        if !scheduled && hm.add_todo(&title, Some(entry_id)).is_ok() {
            todos += 1;
        }
    }

    if events > 0 || todos > 0 {
        let _ = app.emit("auto-organized", AutoOrganizedEvent { events, todos });
        info!("Meeting {entry_id}: auto-created {events} events and {todos} todos");
    }
    Ok(())
}
