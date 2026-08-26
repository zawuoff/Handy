//! Meeting radar: notices when a call seems to be happening and offers to
//! record it.
//!
//! Detection is deliberately cheap and opt-in (`settings.meeting_radar_enabled`,
//! off by default): every 10 seconds — and only while the radar is enabled and
//! nothing is being recorded — two quick `pactl` listings check whether some
//! other application is capturing the microphone while audio is playing.
//! Two consecutive positives fire one desktop notification (with a Record
//! action where the notification server supports it); the radar then stays
//! quiet until that call ends plus a cooldown, so it never nags.

#![cfg(target_os = "linux")]

use crate::managers::audio::AudioRecordingManager;
use crate::settings::get_settings;
use log::{debug, info};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const TICK: Duration = Duration::from_secs(10);
const DISABLED_TICK: Duration = Duration::from_secs(30);
const COOLDOWN: Duration = Duration::from_secs(300);
/// Consecutive positive ticks required before prompting (debounce).
const CONFIRM_TICKS: u32 = 2;

/// Names that identify our own audio streams, to be ignored.
const SELF_NAMES: [&str; 2] = ["handy", "noted"];

fn pactl_blocks(kind: &str) -> Vec<String> {
    let Ok(output) = Command::new("pactl").args(["list", kind]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .split("\n\n")
        .map(str::to_string)
        .collect()
}

fn block_is_foreign(block: &str) -> bool {
    let lower = block.to_lowercase();
    !SELF_NAMES.iter().any(|name| {
        lower.contains(&format!("application.name = \"{name}"))
            || lower.contains(&format!("application.process.binary = \"{name}"))
    })
}

/// True when another app captures the mic while (uncorked) audio plays —
/// the signature of an active call.
fn call_detected() -> bool {
    let capturing = pactl_blocks("source-outputs")
        .iter()
        .filter(|block| block.contains("Source Output #"))
        .filter(|block| !block.contains(".monitor"))
        .any(|block| block_is_foreign(block));
    if !capturing {
        return false;
    }
    pactl_blocks("sink-inputs")
        .iter()
        .filter(|block| block.contains("Sink Input #"))
        .filter(|block| !block.to_lowercase().contains("corked: yes"))
        .any(|block| block_is_foreign(block))
}

/// Show the prompt; returns true when the user chose Record. Uses
/// notification actions when libnotify supports them, otherwise a plain
/// notification (informational only).
fn prompt_to_record(title: &str, action_label: &str) -> bool {
    let with_action = Command::new("notify-send")
        .args([
            "--app-name=Noted",
            "-t",
            "15000",
            "-A",
            &format!("record={action_label}"),
            title,
        ])
        .output();
    match with_action {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim() == "record"
        }
        _ => {
            // Older notify-send without -A: still surface the hint.
            let _ = Command::new("notify-send")
                .args(["--app-name=Noted", "-t", "10000", title])
                .status();
            false
        }
    }
}

/// Spawn the radar thread. Runs for the app's lifetime; each tick is two
/// small subprocess calls at most.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mut positives = 0u32;
        let mut prompted_for_current_call = false;
        let mut cooldown_until: Option<Instant> = None;

        loop {
            let settings = get_settings(&app);
            if !settings.meeting_radar_enabled {
                positives = 0;
                prompted_for_current_call = false;
                std::thread::sleep(DISABLED_TICK);
                continue;
            }

            let busy = app
                .try_state::<Arc<AudioRecordingManager>>()
                .is_some_and(|rm| rm.is_recording());
            let meeting_active = app
                .try_state::<crate::meeting::MeetingSession>()
                .is_some_and(|session| session.is_active());

            if !busy && !meeting_active {
                if call_detected() {
                    positives += 1;
                    let in_cooldown = cooldown_until.is_some_and(|until| Instant::now() < until);
                    if positives >= CONFIRM_TICKS && !prompted_for_current_call && !in_cooldown {
                        prompted_for_current_call = true;
                        cooldown_until = Some(Instant::now() + COOLDOWN);
                        info!("Meeting radar: call detected, prompting");
                        let strings = crate::tray_i18n::get_tray_translations(Some(
                            settings.app_language.clone(),
                        ));
                        let title = if strings.radar_prompt.is_empty() {
                            "A call seems to be happening — record it as a meeting?".to_string()
                        } else {
                            strings.radar_prompt
                        };
                        let action = if strings.radar_record.is_empty() {
                            "Record".to_string()
                        } else {
                            strings.radar_record
                        };
                        // The prompt blocks this thread until the notification
                        // is clicked or dismissed — which is fine, the radar
                        // has nothing else to do meanwhile.
                        if prompt_to_record(&title, &action) {
                            info!("Meeting radar: user chose to record");
                            crate::signal_handle::send_transcription_input(
                                &app, "meeting", "radar",
                            );
                        }
                    }
                } else {
                    if positives > 0 {
                        debug!("Meeting radar: call signature gone");
                    }
                    positives = 0;
                    prompted_for_current_call = false;
                }
            }

            std::thread::sleep(TICK);
        }
    });
}
