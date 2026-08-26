//! Speaker separation for meeting sessions.
//!
//! A streaming Sortformer diarizer (NVIDIA `diar_streaming_sortformer_4spk`,
//! GGUF via transcribe-cpp) runs alongside the meeting's ASR stream, fed the
//! same audio. It emits speaker turns — who spoke when, up to 4 speakers,
//! numbered by arrival order — which are aligned with the ASR segments by
//! time. It produces no text and never restarts mid-meeting, so speaker
//! identities stay stable across the ASR stream's pause resets.

use anyhow::Result;
use futures_util::StreamExt;
use log::{info, warn};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};
use transcribe_cpp::{
    Model, ModelOptions, RunExtension, RunOptions, Session, SortformerStreamOptions,
    SpeakerSegment, StreamOptions,
};

pub const DIARIZER_FILENAME: &str = "diar_streaming_sortformer_4spk-v2.1-Q8_0.gguf";
const DIARIZER_URL: &str = "https://huggingface.co/handy-computer/diar_streaming_sortformer_4spk-v2.1-gguf/resolve/main/diar_streaming_sortformer_4spk-v2.1-Q8_0.gguf";

pub fn diarizer_model_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(crate::portable::app_data_dir(app)
        .map_err(|e| anyhow::anyhow!("Failed to get app data dir: {e}"))?
        .join("models")
        .join(DIARIZER_FILENAME))
}

/// Load the diarizer into its own session (CPU/GPU per library default).
/// The caller owns the session; streams are begun with [`diarizer_options`].
pub fn load_diarizer_session(app: &AppHandle) -> Option<Session> {
    let path = diarizer_model_path(app).ok()?;
    if !path.exists() {
        return None;
    }
    let model = match Model::load_with(&path, &ModelOptions::default()) {
        Ok(m) => m,
        Err(e) => {
            warn!("Failed to load diarizer model: {e}");
            return None;
        }
    };
    match model.session() {
        Ok(session) => {
            info!("Diarizer loaded (backend '{}')", model.backend());
            Some(session)
        }
        Err(e) => {
            warn!("Failed to create diarizer session: {e}");
            None
        }
    }
}

/// Run + stream options for a diarizer stream.
pub fn diarizer_options() -> (RunOptions, StreamOptions) {
    let run = RunOptions {
        family: Some(RunExtension::Sortformer(SortformerStreamOptions {
            preset: None,
        })),
        ..Default::default()
    };
    (run, StreamOptions::default())
}

/// The speaker who talked most during `[t0_ms, t1_ms]`, or `None` when the
/// diarizer has nothing overlapping that span.
pub fn dominant_speaker(segments: &[SpeakerSegment], t0_ms: i64, t1_ms: i64) -> Option<i32> {
    let mut talk_ms: std::collections::HashMap<i32, i64> = std::collections::HashMap::new();
    for seg in segments {
        let overlap = seg.t1_ms.min(t1_ms) - seg.t0_ms.max(t0_ms);
        if overlap > 0 && seg.speaker_id > 0 {
            *talk_ms.entry(seg.speaker_id).or_insert(0) += overlap;
        }
    }
    // Ties go to the earlier-arriving (lower-numbered) speaker so the
    // result is deterministic.
    talk_ms
        .into_iter()
        .max_by_key(|(speaker, ms)| (*ms, -speaker))
        .map(|(speaker, _)| speaker)
}

/// The most recently active speaker at or before `now_ms`.
pub fn latest_speaker(segments: &[SpeakerSegment], now_ms: i64) -> Option<i32> {
    segments
        .iter()
        .filter(|seg| seg.speaker_id > 0 && seg.t0_ms <= now_ms)
        .max_by_key(|seg| seg.t0_ms)
        .map(|seg| seg.speaker_id)
}

/// Rewrite "Speaker N:" labels in a stored transcript with the names the
/// user assigned during the meeting. Names typed mid-meeting apply to the
/// whole transcript, including turns recorded before they were entered.
pub fn apply_speaker_names(
    transcript: &str,
    names: &std::collections::HashMap<i32, String>,
) -> String {
    let mut out = transcript.to_string();
    for (speaker, name) in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        out = out.replace(&format!("Speaker {speaker}:"), &format!("{name}:"));
    }
    out
}

#[derive(Clone, Serialize)]
struct DiarizerDownloadEvent {
    progress: f32,
    done: bool,
    error: Option<String>,
}

fn emit_progress(app: &AppHandle, progress: f32, done: bool, error: Option<String>) {
    let _ = app.emit(
        "diarizer-download",
        DiarizerDownloadEvent {
            progress,
            done,
            error,
        },
    );
}

/// Whether the speaker-separation model is on disk and ready to use.
#[tauri::command]
#[specta::specta]
pub fn is_diarizer_ready(app: AppHandle) -> Result<bool, String> {
    Ok(diarizer_model_path(&app)
        .map_err(|e| e.to_string())?
        .exists())
}

/// Download the speaker-separation model (~139 MB). Progress is emitted as
/// "diarizer-download" events; safe to call when already downloaded.
#[tauri::command]
#[specta::specta]
pub async fn download_diarizer_model(app: AppHandle) -> Result<(), String> {
    let path = diarizer_model_path(&app).map_err(|e| e.to_string())?;
    if path.exists() {
        emit_progress(&app, 1.0, true, None);
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let partial = path.with_extension("gguf.partial");
    let result = download_to(&app, &partial).await;
    match result {
        Ok(()) => {
            std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
            info!("Diarizer model downloaded to {}", path.display());
            emit_progress(&app, 1.0, true, None);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            emit_progress(&app, 0.0, true, Some(e.to_string()));
            Err(e.to_string())
        }
    }
}

async fn download_to(app: &AppHandle, dest: &PathBuf) -> Result<()> {
    let response = reqwest::get(DIARIZER_URL).await?.error_for_status()?;
    let total = response.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest)?;
    let mut downloaded: u64 = 0;
    let mut last_emitted: f32 = -1.0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        std::io::Write::write_all(&mut file, &chunk)?;
        downloaded += chunk.len() as u64;
        if total > 0 {
            let progress = downloaded as f32 / total as f32;
            if progress - last_emitted >= 0.01 {
                last_emitted = progress;
                emit_progress(app, progress, false, None);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(t0: i64, t1: i64, speaker: i32) -> SpeakerSegment {
        SpeakerSegment {
            t0_ms: t0,
            t1_ms: t1,
            speaker_id: speaker,
            p: 1.0,
        }
    }

    #[test]
    fn dominant_speaker_picks_most_overlap() {
        let segs = [seg(0, 1000, 1), seg(1000, 5000, 2)];
        assert_eq!(dominant_speaker(&segs, 0, 3000), Some(2));
        assert_eq!(dominant_speaker(&segs, 0, 1500), Some(1));
        // Exact tie: the earlier-arriving speaker wins deterministically.
        assert_eq!(dominant_speaker(&segs, 0, 2000), Some(1));
        assert_eq!(dominant_speaker(&segs, 6000, 7000), None);
    }

    #[test]
    fn apply_speaker_names_rewrites_all_labels() {
        let names =
            std::collections::HashMap::from([(1, "Nikki".to_string()), (2, "Sam".to_string())]);
        let transcript = "Speaker 1: hello\n\nSpeaker 2: hi\n\nSpeaker 1: bye\n\nSpeaker 3: hm";
        assert_eq!(
            apply_speaker_names(transcript, &names),
            "Nikki: hello\n\nSam: hi\n\nNikki: bye\n\nSpeaker 3: hm"
        );
    }

    #[test]
    fn latest_speaker_is_most_recent_turn() {
        let segs = [seg(0, 1000, 1), seg(1200, 3000, 3)];
        assert_eq!(latest_speaker(&segs, 5000), Some(3));
        assert_eq!(latest_speaker(&segs, 500), Some(1));
        assert_eq!(latest_speaker(&[], 500), None);
    }
}
