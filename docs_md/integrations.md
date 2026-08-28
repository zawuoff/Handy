# Integrations

## Composio → Google Docs (`composio.rs`)

Settings → Integrations holds a Composio API key (`SecretString` in
settings — never log it) plus a Connect button for Google Docs. Connect
lazily creates a Composio auth config (`use_composio_managed_auth`,
toolkit `googledocs`) and a connected account, opens the returned OAuth URL
in the browser, and the UI polls `get_gdocs_connection_status` until
`ACTIVE`. Ids persist in settings (`composio_gdocs_auth_config_id` /
`composio_gdocs_account_id`).

Sync (per-note chip in `NoteDetail`, "Sync all" button on the notes
library) sends the note body the user sees — `user_notes` if edited, else
`ai_notes` — via `POST /tools/execute/GOOGLEDOCS_CREATE_DOCUMENT_MARKDOWN`
(first time; the doc id is stored in the `gdoc_id` column) or
`…UPDATE_DOCUMENT_MARKDOWN` (re-sync updates the same doc — no duplicates).
Guards: per-entry `SYNC_IN_FLIGHT` set + a `SYNC_ALL_RUNNING` flag. Tool
slugs/argument names live as constants at the top of `composio.rs`; if
Composio renames a response field, `find_string` (a tolerant key probe)
is the place to look.

Post-hoc speaker rename lives next door: a saved note whose transcript
still has `Speaker N:` labels shows a "Name speakers" chip; saving calls
`rename_note_speakers`, which regex-replaces `\bSpeaker (\d+)\b` (prose and
bold mentions included) across `transcription_text`, `ai_notes` **and**
`user_notes` via `diarization::apply_speaker_names`.

## Composio → Gmail (`composio.rs`, `notes.rs`)

Same connect machinery, generalized: `connect_composio_toolkit(toolkit)` /
`get_composio_connection_status(toolkit)` handle both `googledocs` and
`gmail` (per-toolkit id fields in settings; `get_toolkit_ids` /
`store_toolkit_ids` map slugs to fields — a new toolkit is one more match
arm plus a `ConnectRow` in `IntegrationsSettings.tsx`).

Two features, both draft-only — **Gmail drafts are never sent by the app**:

- **"Draft email" chip** on a note (`draft_note_email`): a Gmail draft with
  the note body (+ the Google Doc link when synced), addressed to the user
  themself as a placeholder (`GMAIL_GET_PROFILE` supplies the address).
- **Email-commitment pass** (`notes.rs::draft_email_commitments`): a third
  LLM pass after notes + action items — finds promises to email someone
  ("sure, I'll email you the quotation") in the transcript and creates one
  Gmail draft each (≤5). Guarded exactly like action items: an appended
  `email_drafts_organized` column with backfill-to-1 for pre-existing
  meetings, claimed atomically via `try_claim_email_drafts` — the
  connected-check runs BEFORE the claim so meetings recorded before Gmail
  was connected can still draft on a later regenerate. UI toast via the
  `email-drafts` event (listener in `App.tsx`).

## The task key (`tasks.rs`)

A fourth transcribe binding, `execute_task` (default `ctrl+alt+space`;
`option+ctrl+space` on macOS). It records like dictation but the transcript
is routed to `tasks::spawn_execute` instead of being pasted
(`TranscriptionOutput::ExecuteTask` in `actions.rs`; the id is listed in
`is_transcribe_binding` — forgetting that for future bindings bypasses the
coordinator, see the trap note in the shortcut code). One LLM call maps the
speech to JSON actions: todo (`hm.add_todo`), calendar event
(`notes::create_calendar_event`), or email via Composio Gmail — **draft by
default; actually sends only when the user clearly said "send" AND the
recipient resolved**. Recipients resolve: spoken address > saved contact
(`settings.gmail_contacts`, name→email, managed under Settings →
Integrations → Gmail) > the user's own address; an unresolved name
downgrades a send to a draft. `settings.gmail_signature_name` feeds the
sign-off. Outcome (per-action ✓/✗ lines, or the whole-run error) is always
surfaced as a `notify-send` desktop notification, titled via the
build-generated tray strings `tasks_done`/`tasks_failed` (`tray.tasksDone` /
`tray.tasksFailed` in the locale files). The raw command transcript is
archived to history like a dictation.

## MCP server (`mcp.rs`) — Claude / Codex read the notes

The app binary doubles as a stdio MCP server: `handy --mcp-serve` speaks
newline-delimited JSON-RPC 2.0, protocol `2024-11-05`, hand-rolled (no SDK).
It opens `history.db` **read-only** (`SQLITE_OPEN_READ_ONLY`) and resolves the
app data dir itself via the `dirs` crate + `com.pais.handy`, so it works
without the GUI running.

Tools:

- `get_current_meeting` — reads `live_meeting.json` (30 s staleness guard);
  returns elapsed time, the user's live jottings ("Notes the user has taken
  so far"), and the live transcript. This is how "what is my meeting about
  right now?" works mid-meeting.
- `list_meetings`, `get_meeting` — the notes library.
- `search_meetings` — same keyword search as Ask.

Registration (already done on the user's machine):

- Wrapper script `scripts/noted-mcp.sh` (prefers `/usr/bin/handy`).
- Claude Code: registered at user scope under the name `noted`.
- Codex: entry in `~/.codex/config.toml`.

If you change tool schemas, remember both clients cache nothing — but the
wrapper must keep pointing at the installed binary, not a dev build.

## GNOME Calendar (Evolution Data Server)

Two directions, both via `gdbus` (no extra crates):

- **Read** (`calendar.rs`): `get_upcoming_events(hours)` opens the calendar
  (`OpenCalendar` / `GetObjectList` with an `occur-in-time-range` s-expression),
  parses the gvariant strings and VEVENTs (floating times, `Z`, `TZID`,
  all-day). Powers Home's "Coming up" cards; each card's Record button starts
  a meeting and pre-names the resulting entry after the event
  (`pendingMeetingTitle` in `App.tsx`).
- **Write**: always through the user's own `~/.local/bin/cal-add` script
  (bash + gdbus `CreateObjects`, GNU `date` parsing —
  `cal-add "Title" "next sunday 14:00" 60`). Used by meeting action items and
  voice commands. Do not modify this script; it's the user's.

## Meeting radar (`radar.rs`, Linux-only)

Opt-in (`meeting_radar_enabled`). A 10 s tick (30 s when disabled) runs
`pactl` and looks for an app recording the mic (`source-outputs`) while also
playing audio (uncorked `sink-inputs`), excluding Noted itself — i.e. "you
look like you're in a call". After a 2-tick debounce it sends a
`notify-send -A record` desktop notification whose action starts a meeting;
5-minute cooldown. Strings come from `tray_i18n`
(`radar_prompt` / `radar_record`).

## CLI / single instance

Flags in `cli.rs` (see also upstream `AGENTS.md`): `--toggle-transcription`,
`--toggle-post-process`, `--toggle-meeting`, `--cancel`, `--start-hidden`,
`--no-tray`, `--debug`, plus `--mcp-serve` (fork addition) and
`--transcribe-file <wav>` for offline testing of a model against a file.
Remote-control flags work by launching a second instance that forwards args
to the running one via `tauri_plugin_single_instance` and exits. CLI flags
are runtime-only overrides — they never modify persisted settings.
