# Data & storage

App data dir (Linux): `~/.local/share/com.pais.handy/`

```
history.db                  SQLite (rusqlite + rusqlite_migration)
settings_store.json         tauri-plugin-store settings (AppSettings)
live_meeting.json           live-meeting snapshot for MCP (only while a meeting runs)
models/                     ASR GGUFs/ONNX + diar_streaming_sortformer_4spk-v2.1-Q8_0.gguf
recordings/                 WAVs of saved entries
logs/handy.log              the log file — first stop for any debugging
```

The identifier `com.pais.handy` is **frozen** — changing it would orphan all
of the user's data (see gotchas).

## SQLite schema (`managers/history.rs`)

### ⚠️ THE migration rule

`rusqlite_migration` identifies migrations by **position in the list**.
A new migration must **ALWAYS be appended at the END of the `MIGRATIONS`
vec** — never inserted in the middle, never reordered. Inserting mid-list
silently skips the new migration on any DB that already passed that index
(this bug happened once and was caught only by manual `PRAGMA user_version`
inspection). When touching migrations, verify against a copy of the real DB:
`PRAGMA user_version;` and `PRAGMA table_info(...)`.

### Current migration sequence (order matters — do not renumber)

1. Base `transcription_history` table
2. `+ post_processed_text`
3. `+ post_process_prompt`
4. `+ post_process_requested`
5. `+ source` (`'dictation' | 'meeting'`)
6. `+ ai_notes`
7. `+ user_notes`
8. `todos` table
9. `+ action_items_organized` (with backfill) — the atomic-claim flag
10. `ask_sessions` table
11. `+ live_notes` — user's in-meeting jottings (internal; not on
    `HistoryEntry`)

### Tables (conceptual)

- **transcription_history** — every dictation and meeting.
  `HistoryEntry { id, file_name, timestamp, title, transcription_text, saved,
post_processed_text?, source, ai_notes?, user_notes? }`. Meeting entries are
  `saved = 1` from the start so retention cleanup never deletes a long
  recording; `history_limit` retention only trims unsaved dictations.
  The note document shown in the UI is `user_notes ?? ai_notes` (the editor
  writes `user_notes`; regenerating rewrites `ai_notes` and the UI follows
  only if the user hasn't typed).
- **todos** — `{ id, title, done, created_at, source_entry_id? }` with CRUD.
- **ask_sessions** — `{ id, query, answer?, created_at, provider_id? }`.

Search: `search_meeting_notes(query)` — see
[notes-and-ai.md](notes-and-ai.md#ask-your-notes-askrs-askviewtsx) for the
keyword ranking design.

## Settings (`settings.rs` + `settings_store.json`)

`AppSettings` is one big struct; every field needs `#[serde(default…)]` so
old stores load. Highlights beyond upstream Handy:

- `meeting_notes_prompt` — user-editable; **default changes need a
  recognizer migration** (two precedents in `load`/migration code: the
  plain-text→markdown swap and the same-language→English-only swap).
- `diarization_enabled` — speaker separation toggle.
- `meeting_radar_enabled` — call-detection notifications.
- `post_process_providers / _api_keys / _models / _prompts` — LLM config.
- Setters live in `shortcut/mod.rs` as `change_*_setting` commands; the
  frontend maps keys to them in `stores/settingsStore.ts`.

Corrupt stores are salvaged field-by-field (see `settings::tests::salvage_*`).
Backups like `settings_store.json.bak-*` may exist in the data dir.

## live_meeting.json

Written each ticker second while a meeting runs; deleted on end and at app
startup (staleness guard: MCP ignores it if `updated_unix` is >30 s old).

```json
{ "active": true, "streaming": true, "started_unix": …, "updated_unix": …,
  "transcript": "…", "notes": "user's live jottings" }
```

## Frontend persistence

- `localStorage["noted.askProvider"]` — ask-view provider choice.
- Zustand settings store mirrors `AppSettings` reactively.
