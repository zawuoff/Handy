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
    "add a task",
    "add task",
    "create a task",
    "create task",
    "new task",
    "remind me to",
    "add a reminder",
    "add reminder",
    "set a reminder",
];

const EVENT_TRIGGERS: &[&str] = &[
    "add an event",
    "add event",
    "new event",
    "create an event",
    "create event",
    "set an event",
    "set event",
    "set up an event",
    "schedule an event",
    "schedule event",
    "add to my calendar",
    "add to calendar",
    "add to the calendar",
    "put on my calendar",
    "put it on my calendar",
];

/// Polite filler people naturally say before a command ("can you add an
/// event…"). Stripped repeatedly before trigger matching.
const LEAD_INS: &[&str] = &[
    "can you",
    "could you",
    "would you",
    "please",
    "okay",
    "hey",
    "ok",
];

#[derive(Debug, PartialEq, Eq)]
pub enum VoiceCommand {
    Todo { title: String },
    Event { text: String },
}

/// True when `text` starts with `prefix` (case-insensitive) at a word
/// boundary — "add todo:" matches, "add todos" does not.
fn starts_with_word(text: &str, prefix: &str) -> bool {
    let lower = text.to_lowercase();
    lower.starts_with(prefix)
        && !lower[prefix.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric())
}

/// Peel any number of polite lead-ins off the front: "okay, can you please
/// add an event…" → "add an event…". Lead-ins are ASCII, so byte offsets
/// line up with the original text.
fn strip_lead_ins(text: &str) -> &str {
    let mut rest = text.trim();
    loop {
        let before = rest;
        for lead in LEAD_INS {
            if starts_with_word(rest, lead) {
                rest = rest[lead.len()..]
                    .trim_start_matches([':', ',', '-', '—', ' ', '.'])
                    .trim();
            }
        }
        if rest == before {
            return rest;
        }
    }
}

/// Strip the leading trigger phrase (case-insensitive) and tidy the rest.
/// Returns None when the remainder is empty — "add todo" alone is not a task.
fn strip_trigger<'a>(text: &'a str, triggers: &[&str]) -> Option<&'a str> {
    for trigger in triggers {
        if starts_with_word(text, trigger) {
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
    let text = strip_lead_ins(text.trim());
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
    // Date-first phrasing: "on Sunday, update the website" — everything up
    // to the first comma is the date, the rest is the task.
    if let Some(comma) = text.find(',') {
        let head = tidy(&text[..comma]);
        let tail = tidy(&text[comma + 1..]);
        let when = ["on ", "at ", "for "]
            .iter()
            .find(|prep| starts_with_word(&head, prep.trim_end()))
            .map(|prep| tidy(&head[prep.len()..]))
            .or_else(|| {
                ["tomorrow", "today", "tonight", "next", "this"]
                    .iter()
                    .any(|word| starts_with_word(&head, word))
                    .then(|| head.clone())
            });
        if let Some(when) = when {
            if !when.is_empty() && !tail.is_empty() {
                candidates.push((tail, when));
            }
        }
    }
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
    fn polite_lead_ins_are_stripped() {
        for text in [
            "Can you add an event, on Sunday, update Nikki's website",
            "Okay, please set an event on Sunday, update Nikki's website",
            "Hey, could you create an event: on Sunday, update Nikki's website",
        ] {
            assert!(
                matches!(parse(text), Some(VoiceCommand::Event { .. })),
                "expected event for '{text}'"
            );
        }
        assert!(matches!(
            parse("Please remind me to buy milk"),
            Some(VoiceCommand::Todo { .. })
        ));
    }

    #[test]
    fn trigger_needs_word_boundary() {
        // "add todos" must not match the "add todo" trigger.
        assert_eq!(parse("add todos to my day, lots of them"), None);
    }

    #[test]
    fn event_split_handles_date_first_phrasing() {
        let candidates =
            event_split_candidates("on Sunday, update Nikki's website, the copy of the website");
        assert_eq!(
            candidates[0],
            (
                "update Nikki's website, the copy of the website".to_string(),
                "Sunday".to_string()
            )
        );
        let candidates = event_split_candidates("tomorrow, call the dentist");
        assert_eq!(
            candidates[0],
            ("call the dentist".to_string(), "tomorrow".to_string())
        );
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
