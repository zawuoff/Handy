# Meeting mode

Meeting mode records a long session from the microphone, shows a live
transcript, and saves everything to the local library instead of pasting.
It is triggered by the `meeting` shortcut (default `ctrl+alt+m`), the
Start/Stop button on Home, the tray menu, a calendar card's Record button, or
`handy --toggle-meeting`.

## Lifecycle

1. **Start** — `actions.rs` start path runs with
   `TranscriptionOutput::SaveToHistory`. Differences from dictation: the mic
   is never muted, the Escape-cancel shortcut is not registered, and
   `meeting::begin_session` is called. If the selected model supports
   streaming, `TranscriptionManager::start_stream(diarize)` begins the live
   stream (`diarize` is true only for meetings with the speaker-separation
   setting on **and** the diarizer model on disk).
2. **During** — audio frames flow to the stream worker; committed/tentative
   text is emitted as `StreamTextEvent` and mirrored into `MeetingSession`
   (tray readout `● 12:34 · N words`, copy-so-far, live snapshot file).
   The frontend shows `LiveMeetingView`.
3. **Stop** — `end_session` stashes captions state, the user's live notes and
   speaker names into `PENDING_*` statics, then the normal stop pipeline
   finalizes the stream (or falls back to batch transcription), post-processes
   text (custom words etc.), applies speaker names to the transcript, and
   calls `history.save_meeting_entry(file_name, text, live_notes)`.
4. **After save** — a `notes::spawn_generation` kicks off AI notes; when notes
   finish, the action-item pass may create todos/calendar events. Toasts keep
   the user informed (`meeting-saved`, `notes-status`, `meeting-save-failed`
   with clipboard rescue of the transcript if the DB write failed).

State machine notes: `MeetingSession` holds an `ActiveMeeting` struct
(generation counter, start time, streaming flag, committed/tentative text,
captions flag, `live_notes`, `speaker_names`). A 1 s ticker updates the tray
and persists `live_meeting.json` (see [integrations.md](integrations.md)).

## The live view (`LiveMeetingView.tsx`)

Matches Paper mockup 3 ("3 · Live meeting"):

- **Top bar**: breadcrumb, `● Recording · mm:ss | Stop` pill, Captions toggle
  (only when streaming). The Stop button flushes the notes debounce before
  toggling so no keystrokes are lost.
- **Left pane — notes pad**: serif "Meeting notes" heading, hint text, and a
  borderless auto-growing textarea. Content is debounced 500 ms to
  `set_live_meeting_notes` and cached module-level keyed by
  `meeting.startedAtMs` so remounts within one meeting keep the text. These
  jottings are stored in the DB (`live_notes` column) and **anchor the AI
  note generation** (see [notes-and-ai.md](notes-and-ai.md)).
- **Right rail — live transcript** (340 px, `bg-card`, bordered): pulsing dot
  header, the transcript, the speakers button/card, privacy footer.
  With speaker separation on, the transcript renders as turn blocks
  (`Speaker 2 · 3:41` + text) from `StreamTextEvent.turns`; without it, flat
  committed+tentative text.
- **Speakers card**: visible as soon as any turn exists. Shows
  "Identifying speakers…" (disabled) until the diarizer confirms an id, then
  "Name speakers (N)". Opens an animated card with one input per speaker;
  Enter or Save calls `set_meeting_speaker_name` per id. Names apply to the
  live rendering immediately and to the whole saved transcript at stop.

## Streaming internals (`managers/transcription.rs::run_stream_worker`)

The stream worker leases the loaded engine, begins a transcribe-cpp `Stream`,
and loops over `StreamCmd::{Feed, Finalize, Cancel}` from the router.

### Pause reset (language-lock fix)

Small streaming models condition on their own prior output; once the user's
mixed Hindi/English/Arabic speech commits Devanagari, accented English keeps
decoding as Devanagari **forever** within one stream. Fix: the worker tracks
trailing silence via cheap RMS (`STREAM_SILENCE_RMS = 0.01`); after
`STREAM_PAUSE_RESET_SECS = 1.5` s of silence with committed text present, it
finalizes the current stream and starts a fresh one inside the `'segments`
loop, carrying committed text forward (`join_stream_text`). A fresh decode
context re-detects language from audio alone, so the transcript recovers at
every natural pause. Dictation streams share this code path harmlessly.

### Speaker separation

Model: `handy-computer/diar_streaming_sortformer_4spk-v2.1-gguf` (Q8_0,
~139 MB, NVIDIA Sortformer, up to **4 speakers**, ids in arrival order).
Downloaded by `diarization.rs::download_diarizer_model` into the models dir
(progress via `diarizer-download` events; UI in
`components/settings/SpeakerSeparation.tsx`); gated by the
`diarization_enabled` setting.

**Crucial constraint**: in transcribe-cpp 0.2.0 the sortformer family has
**no stream API** (`stream begin: not implemented`). So `Diarizer` is a
worker thread that:

- accumulates the full meeting audio (16 kHz mono f32 — ~230 MB/hour; a known
  deferred optimization),
- re-runs `session.run()` over **all** audio on an adaptive cadence
  (idle: ≥10 s **and** ≥10 % new audio; a `RunNow` nudge at each ASR pause
  reset: ≥5 s and ≥5 % new). Re-running over the same growing prefix keeps
  arrival-order speaker ids stable between passes,
- publishes the latest `SpeakerSegment` list behind a mutex,
- on `finalize_wait(10s)` does one definitive full pass (the stream finalize
  handshake upstream budgets 30 s total, so the wait is capped; on timeout
  the latest finished result is used).

**Attribution** is deliberately re-derived on every read: the worker records
each finished ASR stream segment as an `AsrRecord { offset_ms, end_ms,
segments, full_text }` (offset maps segment-local times onto the diarizer's
meeting-global clock, tracked as `total_fed_ms`). `build_turns(records,
diar_segments)` assigns each timed ASR segment the overlap-dominant speaker
(`dominant_speaker`, ties → lower id), falls back to the span-dominant or
previous speaker for untimed rows, and merges consecutive same-speaker turns.
Because it is recomputed on every emit, **fresher diarization retroactively
corrects earlier labels** in the live view. The in-progress (not yet
finalized) text is shown under `latest_speaker` and corrected at the next
pause reset. A 2 s tick in the feed arm re-emits even when no new text
arrives so labels refresh during silence.

The final meeting transcript is `format_turns(...)` — paragraphs like
`Speaker 2: …` — and `actions.rs` rewrites labels to user-assigned names via
`diarization::apply_speaker_names` (retroactive across the whole transcript).

### Events

`StreamTextEvent { committed, tentative, turns: SpeakerTurn[] }` where
`SpeakerTurn { speaker: i32 (1-based, 0 = unattributed), text, t_ms }`.
Consumed by `LiveMeetingView` and the recording overlay (which ignores turns).

## Known limitations / deferred

- Whole meeting audio is held in RAM twice (recorder + diarizer copies).
- 4-speaker cap (model limit); a 5th voice folds into the closest id.
- Speaker labels lag words by up to one diarizer pass; on very long meetings
  the adaptive cadence stretches the lag (final save is always full-accuracy).
- No system-audio (Me/Them) capture yet — mic-only. This is the biggest
  wanted future feature ("the crown").
- Per-mode models (e.g. Parakeet for dictation, a multilingual streamer for
  meetings) is wanted but not built.
