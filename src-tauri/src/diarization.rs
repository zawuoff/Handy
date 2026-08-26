//! Speaker separation for meeting sessions.
//!
//! A Sortformer diarizer (NVIDIA `diar_streaming_sortformer_4spk`, GGUF via
//! transcribe-cpp) runs alongside the meeting's ASR, fed the same audio. It
//! emits speaker turns — who spoke when, up to 4 speakers, numbered by
//! arrival order — which are aligned with the ASR segments by time. It
//! produces no text.

use anyhow::Result;
use futures_util::StreamExt;
use log::{info, warn};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};
use transcribe_cpp::{
    Model, ModelOptions, RunExtension, RunOptions, Session, SortformerStreamOptions, SpeakerSegment,
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
fn load_diarizer_session(app: &AppHandle) -> Option<Session> {
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

fn diarizer_run_options() -> RunOptions {
    RunOptions {
        family: Some(RunExtension::Sortformer(SortformerStreamOptions {
            preset: None,
        })),
        ..Default::default()
    }
}

enum DiarCmd {
    Feed(Vec<f32>),
    /// Re-run soon if enough new audio arrived (sent at ASR pause resets).
    RunNow,
    /// Run one last time over everything and reply with the result.
    Final(std::sync::mpsc::Sender<Vec<SpeakerSegment>>),
}

/// A meeting-long diarization worker. Sortformer has no incremental stream
/// API in this engine version ("stream begin: not implemented"), so the
/// worker keeps the full meeting audio and re-runs the model over all of it
/// on an adaptive cadence — every run re-derives arrival-order speaker ids
/// from scratch over the same growing prefix, which keeps them stable.
/// Attribution readers take the latest finished result.
pub struct Diarizer {
    tx: std::sync::mpsc::Sender<DiarCmd>,
    segments: std::sync::Arc<std::sync::Mutex<Vec<SpeakerSegment>>>,
}

impl Diarizer {
    /// Load the model and spawn the worker; `None` when the model is not on
    /// disk or fails to load.
    pub fn start(app: &AppHandle) -> Option<Diarizer> {
        let mut session = load_diarizer_session(app)?;
        let (tx, rx) = std::sync::mpsc::channel::<DiarCmd>();
        let segments = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let shared = std::sync::Arc::clone(&segments);
        std::thread::spawn(move || {
            let opts = diarizer_run_options();
            let mut audio: Vec<f32> = Vec::new();
            let mut last_run_len: usize = 0;
            let run = |audio: &Vec<f32>, last_run_len: &mut usize, session: &mut Session| {
                if audio.is_empty() {
                    return;
                }
                let started = std::time::Instant::now();
                match session.run(audio, &opts) {
                    Ok(transcript) => {
                        info!(
                            "Diarizer run: {:.0}s audio, {} speaker segments, took {:?}",
                            audio.len() as f32 / 16_000.0,
                            transcript.speaker_segments.len(),
                            started.elapsed()
                        );
                        *shared.lock().unwrap() = transcript.speaker_segments;
                    }
                    Err(e) => warn!("Diarizer run failed: {e}"),
                }
                *last_run_len = audio.len();
            };
            loop {
                match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    Ok(DiarCmd::Feed(pcm)) => audio.extend_from_slice(&pcm),
                    Ok(DiarCmd::RunNow) => {
                        // Worth a fresh pass when ≥5s (and ≥5% of the total —
                        // full-audio reruns get expensive on long meetings)
                        // arrived since the last one.
                        let new = audio.len() - last_run_len;
                        if new >= 5 * 16_000 && new * 20 >= audio.len() {
                            run(&audio, &mut last_run_len, &mut session);
                        }
                    }
                    Ok(DiarCmd::Final(reply)) => {
                        // Take any feeds still queued behind us, then run over
                        // everything for the definitive attribution.
                        while let Ok(cmd) = rx.try_recv() {
                            if let DiarCmd::Feed(pcm) = cmd {
                                audio.extend_from_slice(&pcm);
                            }
                        }
                        run(&audio, &mut last_run_len, &mut session);
                        let _ = reply.send(shared.lock().unwrap().clone());
                        break;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Idle cadence: refresh when ≥15s and ≥10% new audio.
                        let new = audio.len() - last_run_len;
                        if new >= 15 * 16_000 && new * 10 >= audio.len() {
                            run(&audio, &mut last_run_len, &mut session);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Some(Diarizer { tx, segments })
    }

    pub fn feed(&self, pcm: &[f32]) {
        let _ = self.tx.send(DiarCmd::Feed(pcm.to_vec()));
    }

    /// Latest finished speaker segments (may lag the newest audio).
    pub fn segments(&self) -> Vec<SpeakerSegment> {
        self.segments.lock().unwrap().clone()
    }

    /// Nudge the worker to refresh soon (called at ASR pause resets).
    pub fn request_run(&self) {
        let _ = self.tx.send(DiarCmd::RunNow);
    }

    /// Final full-audio pass; falls back to the latest finished result when
    /// it does not complete within `timeout` (the stream finalize handshake
    /// has its own 30s budget upstream).
    pub fn finalize_wait(&self, timeout: std::time::Duration) -> Vec<SpeakerSegment> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        if self.tx.send(DiarCmd::Final(reply_tx)).is_err() {
            return self.segments();
        }
        match reply_rx.recv_timeout(timeout) {
            Ok(segments) => segments,
            Err(_) => {
                warn!("Diarizer final pass didn't finish in {timeout:?}; using latest result");
                self.segments()
            }
        }
    }
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
