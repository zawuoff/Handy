//! Voice commands spoken through ordinary dictation.
//!
//! When a dictation's final text starts with a command phrase ("add todo:
//! buy milk", "add event: dentist on friday at 3pm"), the text is executed
//! instead of pasted: todos land in the Todos list, events go to the
//! calendar via the same `cal-add` bridge the meeting extraction uses. An
//! event whose date can't be understood falls back to a todo so nothing
//! spoken is ever lost. Trigger phrases are English-only for now.

use crate::managers::history::HistoryManager;
use log::{debug, info};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

const TODO_TRIGGERS: &[&str] = &[
    "add a todo",
    "add todo",
    "add a to-do",
    "add to-do",
    "add a to do",
    "add to do",
    "new todo",
    "new to-do",
    "create a todo",
    "create todo",
];

const EVENT_TRIGGERS: &[&str] = &[
    "add an event",
    "add event",
    "new event",
    "create an event",
    "create event",
    "add to my calendar",
    "add to calendar",
    "add to the calendar",
];

#[derive(Debug, PartialEq, Eq)]
pub enum VoiceCommand {
    Todo { title: String },
    Event { text: String },
}

/// Strip the leading trigger phrase (case-insensitive) and tidy the rest.
/// Returns None when the remainder is empty — "add todo" alone is not a task.
fn strip_trigger<'a>(text: &'a str, triggers: &[&str]) -> Option<&'a str> {
    let lower = text.to_lowercase();
    for trigger in triggers {
        if lower.starts_with(trigger) {
            // Triggers are ASCII, so byte offsets line up with the original.
            let rest = text[trigger.len()..]
                .trim_start_matches([':', ',', '-', '—', ' ', '.'])
                .trim();
            if !rest.is_empty() {
                return Some(rest);
            }
        }
    }
    None
}

fn tidy(text: &str) -> String {
    text.trim()
        .trim_end_matches(['.', '!', ','])
        .trim()
        .to_string()
}

/// Detect a voice command in a finished dictation. Event triggers are
/// checked first so "add to calendar" never collides with "add to do".
pub fn parse(text: &str) -> Option<VoiceCommand> {
    let text = text.trim();
    if let Some(rest) = strip_trigger(text, EVENT_TRIGGERS) {
        return Some(VoiceCommand::Event { text: tidy(rest) });
    }
    if let Some(rest) = strip_trigger(text, TODO_TRIGGERS) {
        return Some(VoiceCommand::Todo { title: tidy(rest) });
    }
    None
}

/// Candidate (title, when) splits for an event phrase, best-guess first:
/// the rightmost " on " then " at " keeps date tails like
/// "dentist checkup on friday at 3pm" → ("dentist checkup", "friday at 3pm").
fn event_split_candidates(text: &str) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let lower = text.to_lowercase();
    for sep in [" on ", " at ", " for "] {
        if let Some(pos) = lower.rfind(sep) {
            let title = tidy(&text[..pos]);
            let when = tidy(&text[pos + sep.len()..]);
            if !title.is_empty() && !when.is_empty() {
                candidates.push((title, when));
            }
        }
    }
    candidates
}

#[derive(Clone, Serialize)]
struct VoiceCommandEvent {
    kind: &'static str, // "todo" | "event" | "event_fallback_todo"
    title: String,
    ok: bool,
}

fn emit_result(app: &AppHandle, kind: &'static str, title: &str, ok: bool) {
    let _ = app.emit(
        "voice-command",
        VoiceCommandEvent {
            kind,
            title: title.to_string(),
            ok,
        },
    );
}

/// Execute a parsed command. Never panics; the outcome is toasted via the
/// "voice-command" event.
pub fn execute(app: &AppHandle, command: VoiceCommand) {
    let Some(hm) = app.try_state::<Arc<HistoryManager>>() else {
        return;
    };
    match command {
        VoiceCommand::Todo { title } => {
            let ok = hm.add_todo(&title, None).is_ok();
            info!("Voice command: todo '{title}' (ok: {ok})");
            emit_result(app, "todo", &title, ok);
        }
        VoiceCommand::Event { text } => {
            for (title, when) in event_split_candidates(&text) {
                if crate::notes::create_calendar_event(&title, &when, 60).is_ok() {
                    info!("Voice command: event '{title}' at '{when}'");
                    emit_result(app, "event", &title, true);
                    return;
                }
                debug!("Voice event split rejected: '{title}' / '{when}'");
            }
            // No parseable date — keep the thought as a todo instead.
            let ok = hm.add_todo(&text, None).is_ok();
            info!("Voice command: event fell back to todo '{text}' (ok: {ok})");
            emit_result(app, "event_fallback_todo", &text, ok);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_phrases_parse_with_punctuation_and_case() {
        for text in [
            "Add todo: buy milk.",
            "add a todo, buy milk",
            "Add to do - buy milk!",
            "New todo buy milk",
            "ADD TO-DO: Buy milk",
        ] {
            match parse(text) {
                Some(VoiceCommand::Todo { title }) => {
                    assert_eq!(title.to_lowercase(), "buy milk")
                }
                other => panic!("expected todo for '{text}', got {other:?}"),
            }
        }
    }

    #[test]
    fn event_phrases_parse() {
        match parse("Add event: dentist on Friday at 3pm.") {
            Some(VoiceCommand::Event { text }) => {
                assert_eq!(text, "dentist on Friday at 3pm")
            }
            other => panic!("expected event, got {other:?}"),
        }
        assert!(matches!(
            parse("add to calendar lunch with Sam tomorrow at noon"),
            Some(VoiceCommand::Event { .. })
        ));
    }

    #[test]
    fn event_trigger_wins_over_todo_prefix_overlap() {
        // "add to calendar" must not be eaten by the "add to do" trigger.
        assert!(matches!(
            parse("add to calendar standup at 9am"),
            Some(VoiceCommand::Event { .. })
        ));
    }

    #[test]
    fn ordinary_dictation_is_not_a_command() {
        assert_eq!(parse("Hello, how are you?"), None);
        assert_eq!(parse("I need to add todos to my day"), None);
        // A bare trigger with no content is not a command either.
        assert_eq!(parse("add todo"), None);
        assert_eq!(parse("Add event."), None);
    }

    #[test]
    fn event_splits_prefer_rightmost_date_tail() {
        let candidates = event_split_candidates("dentist checkup on friday at 3pm");
        assert_eq!(
            candidates[0],
            ("dentist checkup".to_string(), "friday at 3pm".to_string())
        );
        assert!(candidates.contains(&("dentist checkup on friday".to_string(), "3pm".to_string())));
    }
}
