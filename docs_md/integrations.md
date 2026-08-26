# Integrations

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
