//! Live state for an in-progress meeting session.
//!
//! A meeting session is the span between the meeting shortcut starting a
//! recording and that recording being stopped or discarded. While one is
//! active, the tray shows a meeting menu (timer, live transcript lines,
//! stop/copy/captions/discard actions) and a live readout next to the tray
//! icon; see `tray::update_meeting_readout`. The session text is fed from the
//! streaming transcription worker (`TranscriptionManager::emit_stream_text`),
//! so live text exists only when the active model supports streaming.

use crate::settings::{self, OverlayStyle};
use log::{debug, warn};
use serde::Serialize;
use specta::Type;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// How long the "Copied ✓" confirmation replaces the copy item's label.
const COPY_FLASH: Duration = Duration::from_secs(2);
/// Character budget for the ticker text shown next to the tray icon.
const TITLE_TAIL_CHARS: usize = 40;
/// Character budget per live transcript line in the tray menu.
const MENU_LINE_CHARS: usize = 60;

struct ActiveMeeting {
    generation: u64,
    started_at: Instant,
    streaming: bool,
    committed: String,
    tentative: String,
    captions_visible: bool,
    copied_flash_until: Option<Instant>,
}

/// Everything the tray needs to render the live meeting readout.
pub struct MeetingReadout {
    pub elapsed_label: String,
    /// Tail of the live transcript for the tray-icon label (streaming only).
    pub title_tail: Option<String>,
    /// Up to two rolling transcript lines for the menu (streaming only).
    pub lines: Option<(String, String)>,
    pub captions_visible: bool,
    pub copied_flash: bool,
}

#[derive(Default)]
pub struct MeetingSession {
    inner: Mutex<Option<ActiveMeeting>>,
    /// Monotonic id handed to each session so a stale ticker thread from a
    /// previous session can never outlive its own meeting.
    generation_counter: Mutex<u64>,
}

impl MeetingSession {
    pub fn new() -> Self {
        Self::default()
    }

    fn begin(&self, streaming: bool, captions_visible: bool) -> u64 {
        let generation = {
            let mut counter = self.generation_counter.lock().unwrap();
            *counter += 1;
            *counter
        };
        *self.inner.lock().unwrap() = Some(ActiveMeeting {
            generation,
            started_at: Instant::now(),
            streaming,
            committed: String::new(),
            tentative: String::new(),
            captions_visible,
            copied_flash_until: None,
        });
        generation
    }

    /// Ends the session. Returns whether the captions overlay was visible,
    /// or `None` if no session was active.
    fn take_end(&self) -> Option<bool> {
        self.inner
            .lock()
            .unwrap()
            .take()
            .map(|active| active.captions_visible)
    }

    pub fn is_active(&self) -> bool {
        self.inner.lock().unwrap().is_some()
    }

    fn is_generation_active(&self, generation: u64) -> bool {
        self.inner
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|active| active.generation == generation)
    }

    pub fn is_streaming(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|active| active.streaming)
    }

    /// Called from the streaming transcription worker on every text update.
    /// No-op when no meeting is active (i.e. during ordinary dictation).
    pub fn update_text(&self, committed: &str, tentative: &str) {
        if let Some(active) = self.inner.lock().unwrap().as_mut() {
            active.committed.clear();
            active.committed.push_str(committed);
            active.tentative.clear();
            active.tentative.push_str(tentative);
        }
    }

    /// Full live transcript so far (committed plus tentative words).
    pub fn transcript_snapshot(&self) -> String {
        match self.inner.lock().unwrap().as_ref() {
            Some(active) => {
                let mut text = active.committed.trim_end().to_string();
                let tentative = active.tentative.trim();
                if !tentative.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(tentative);
                }
                text
            }
            None => String::new(),
        }
    }

    fn flash_copied(&self) {
        if let Some(active) = self.inner.lock().unwrap().as_mut() {
            active.copied_flash_until = Some(Instant::now() + COPY_FLASH);
        }
    }

    /// Flips captions visibility; returns the new state, or `None` when no
    /// session is active.
    fn toggle_captions(&self) -> Option<bool> {
        self.inner.lock().unwrap().as_mut().map(|active| {
            active.captions_visible = !active.captions_visible;
            active.captions_visible
        })
    }

    pub fn readout(&self) -> Option<MeetingReadout> {
        let guard = self.inner.lock().unwrap();
        let active = guard.as_ref()?;

        let live = single_line(&format!("{} {}", active.committed, active.tentative));
        let (title_tail, lines) = if active.streaming && !live.is_empty() {
            (
                Some(tail_chars(&live, TITLE_TAIL_CHARS)),
                Some(split_tail_lines(&live, MENU_LINE_CHARS)),
            )
        } else {
            (None, None)
        };

        Some(MeetingReadout {
            elapsed_label: format_elapsed(active.started_at.elapsed()),
            title_tail,
            lines,
            captions_visible: active.captions_visible,
            copied_flash: active
                .copied_flash_until
                .is_some_and(|until| Instant::now() < until),
        })
    }
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Last `max` characters of `text` (on a char boundary), with a leading
/// ellipsis when truncated.
fn tail_chars(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let tail: String = chars[chars.len() - max..].iter().collect();
    format!("…{}", tail)
}

/// The last `2 * per_line` characters split into two rows for the tray menu.
fn split_tail_lines(text: &str, per_line: usize) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len().min(per_line * 2);
    let window = &chars[chars.len() - total..];
    if window.len() <= per_line {
        return (String::new(), window.iter().collect());
    }
    let split = window.len() - per_line;
    let first: String = window[..split].iter().collect();
    let second: String = window[split..].iter().collect();
    let first = if total < chars.len() {
        format!("…{}", first)
    } else {
        first
    };
    (first, second)
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

/// Starts a meeting session: records the session state, switches the tray to
/// the meeting menu, and spawns the once-a-second readout ticker.
pub fn begin_session(app: &AppHandle, streaming: bool) {
    let Some(session) = app.try_state::<MeetingSession>() else {
        return;
    };
    // If the overlay style already shows the live panel for this session,
    // the captions toggle starts in the "visible" position.
    let style = settings::get_settings(app).overlay_style;
    let captions_visible = streaming && style == OverlayStyle::Live;
    let generation = session.begin(streaming, captions_visible);
    debug!("Meeting session {generation} started (streaming: {streaming})");

    // The main window switches to the live-meeting view on this event.
    let _ = app.emit("meeting-started", streaming);
    crate::tray::update_tray_menu(app);

    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            crate::tray::update_meeting_readout(&app);
            std::thread::sleep(Duration::from_secs(1));
            let Some(session) = app.try_state::<MeetingSession>() else {
                return;
            };
            if !session.is_generation_active(generation) {
                // One final pass clears the tray-icon label.
                crate::tray::update_meeting_readout(&app);
                return;
            }
        }
    });
}

/// Ends the meeting session (stop or discard). Restores the overlay-enabled
/// cache if the captions toggle had forced it on, and clears the tray readout.
pub fn end_session(app: &AppHandle) {
    let Some(session) = app.try_state::<MeetingSession>() else {
        return;
    };
    if let Some(captions_were_visible) = session.take_end() {
        debug!("Meeting session ended (captions visible: {captions_were_visible})");
        if captions_were_visible {
            restore_overlay_enabled_cache(app);
        }
        let _ = app.emit("meeting-ended", ());
        crate::tray::update_meeting_readout(app);
    }
}

/// Snapshot of the meeting session for the main window's live view.
#[derive(Clone, Serialize, Type)]
pub struct MeetingState {
    pub active: bool,
    pub streaming: bool,
    pub captions_visible: bool,
    pub elapsed_secs: u32,
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting_state(app: AppHandle) -> Result<MeetingState, String> {
    let session = app
        .try_state::<MeetingSession>()
        .ok_or_else(|| "meeting session state missing".to_string())?;
    let readout = session.readout();
    let elapsed_secs = {
        let guard = session.inner.lock().unwrap();
        guard
            .as_ref()
            .map(|active| active.started_at.elapsed().as_secs().min(u32::MAX as u64) as u32)
            .unwrap_or(0)
    };
    Ok(MeetingState {
        active: readout.is_some(),
        streaming: session.is_streaming(),
        captions_visible: readout.as_ref().is_some_and(|r| r.captions_visible),
        elapsed_secs,
    })
}

/// Start or stop a meeting session from the UI — same toggle edge as the
/// meeting shortcut, so the coordinator lifecycle stays coherent.
#[tauri::command]
#[specta::specta]
pub fn toggle_meeting_session(app: AppHandle) -> Result<(), String> {
    crate::signal_handle::send_transcription_input(&app, "meeting", "UI");
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn toggle_meeting_captions(app: AppHandle) -> Result<(), String> {
    toggle_captions(&app);
    Ok(())
}

/// Tray menu action: copy the live transcript captured so far.
pub fn copy_transcript_so_far(app: &AppHandle) {
    let Some(session) = app.try_state::<MeetingSession>() else {
        return;
    };
    let text = session.transcript_snapshot();
    if text.trim().is_empty() {
        warn!("Meeting transcript is empty so far; nothing to copy.");
        return;
    }
    match crate::clipboard::write_text_to_clipboard(app, &text) {
        Ok(()) => {
            session.flash_copied();
            crate::tray::update_meeting_readout(app);
        }
        Err(err) => log::error!("Failed to copy meeting transcript: {err}"),
    }
}

/// Tray menu action: show or hide the live captions overlay for this session.
///
/// The overlay-enabled cache normally mirrors the overlay_style setting (None
/// disables all overlay traffic — the Linux default). An explicit captions
/// request must override that for the duration, and restore it after.
pub fn toggle_captions(app: &AppHandle) {
    let Some(session) = app.try_state::<MeetingSession>() else {
        return;
    };
    let Some(now_visible) = session.toggle_captions() else {
        return;
    };
    if now_visible {
        crate::overlay::update_overlay_enabled_cache(true);
        crate::overlay::show_captions_overlay_forced(app);
        // The real readiness event fired at session start; the overlay was
        // (re)mounted after it, so re-arm it explicitly.
        crate::overlay::emit_recording_ready(app);
    } else {
        crate::overlay::hide_recording_overlay(app);
        restore_overlay_enabled_cache(app);
    }
    crate::tray::update_meeting_readout(app);
}

fn restore_overlay_enabled_cache(app: &AppHandle) {
    let style = settings::get_settings(app).overlay_style;
    crate::overlay::update_overlay_enabled_cache(style != OverlayStyle::None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_chars_keeps_short_text_and_truncates_long() {
        assert_eq!(tail_chars("hello", 10), "hello");
        assert_eq!(tail_chars("hello world", 5), "…world");
        // Multibyte safety: must split on char boundaries.
        assert_eq!(tail_chars("héllö wörld", 5), "…wörld");
    }

    #[test]
    fn split_tail_lines_fills_second_line_first() {
        let (l1, l2) = split_tail_lines("short", 10);
        assert_eq!(l1, "");
        assert_eq!(l2, "short");

        let (l1, l2) = split_tail_lines("aaaaabbbbb", 5);
        assert_eq!(l1, "aaaaa");
        assert_eq!(l2, "bbbbb");

        let (l1, l2) = split_tail_lines("xxaaaaabbbbb", 5);
        assert_eq!(l1, "…aaaaa");
        assert_eq!(l2, "bbbbb");
    }

    #[test]
    fn elapsed_formats_minutes_and_hours() {
        assert_eq!(format_elapsed(Duration::from_secs(59)), "00:59");
        assert_eq!(format_elapsed(Duration::from_secs(600)), "10:00");
        assert_eq!(format_elapsed(Duration::from_secs(3661)), "1:01:01");
    }

    #[test]
    fn session_text_and_snapshot_roundtrip() {
        let session = MeetingSession::new();
        assert!(session.transcript_snapshot().is_empty());

        session.begin(true, false);
        session.update_text("we agreed on friday", "for the review");
        assert_eq!(
            session.transcript_snapshot(),
            "we agreed on friday for the review"
        );

        let readout = session.readout().expect("active session has a readout");
        assert!(readout.title_tail.is_some());
        assert!(readout.lines.is_some());

        assert_eq!(session.take_end(), Some(false));
        assert!(session.readout().is_none());
    }
}
