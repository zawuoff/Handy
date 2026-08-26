# Notes, AI, todos, ask, voice commands

All LLM work goes through the user's configured **post-process providers**
(`settings.post_process_providers` — OpenAI-compatible HTTP APIs). The user's
active provider is `custom` (an OpenAI-compatible endpoint) with model
`deepseek-v4-flash`. `llm_client.rs` handles chat completions; a cold local
model can take 40–75 s to first token, which is why single-flight guards
exist everywhere.

The word **"AI" is banned from the notes UI** — user-facing copy says
"Enhanced", "Writing notes…", etc.

## Note generation (`notes.rs`)

- Trigger: automatically after a meeting entry is saved, or manually via the
  ↻ regenerate control on a note. `spawn_generation(entry_id)` →
  `generate_and_store`.
- **Single flight**: `IN_FLIGHT: Lazy<Mutex<HashSet<i64>>>` per entry id;
  `get_generating_note_ids` seeds the UI on mount so an already-running job
  shows "Writing notes…" instead of tempting a duplicate click.
- System prompt = `settings.meeting_notes_prompt` (user-editable in
  Settings → Notes). The default demands: Markdown structure
  (`## Summary / Key points / Decisions / Action items`), **strictly English
  output regardless of transcript language** (and normalizing English written
  in other scripts, e.g. Devanagari), **work-focus** (keep substantive
  discussions, drop personal chatter/jokes), no invention, notes only.
  Changing this default requires a `settings.rs` migration recognizing the
  old default text (two such migrations exist already — follow their shape).
- User message: the transcript, prefixed — when the user jotted live notes —
  with "Notes the user personally took during this meeting — treat them as
  the most important signals and build the notes around them: …". Live notes
  come from `history.get_entry_live_notes`.
- Result → `ai_notes` column; `notes-status` events (`generating|done|failed`)
  drive UI state; a `historyUpdatePayload` "updated" event refreshes lists.
- Output is sanitized (`strip_think_block`, invisible chars).

## Action items → todos & calendar (`notes.rs::organize_action_items`)

Second LLM pass over the generated notes returning a JSON array of
`{ title, when, duration_min }`:

- Items **with a date** → `create_calendar_event(title, when, duration_min)`,
  which shells out to the user's own `~/.local/bin/cal-add` script
  (args: `"Title" "<GNU-date string>" [minutes]`; falls back to `cal-add` on
  PATH). The script writes to GNOME's Evolution Data Server "Personal"
  calendar via `gdbus CreateObjects`.
- Items **without a date** → `history.add_todo`.
- **Duplicate protection** (this bit was hard-won): an atomic claim
  `UPDATE transcription_history SET action_items_organized = 1 WHERE id = ?
AND action_items_organized = 0` (proceed only if `rows == 1`), plus
  normalized-title dedup within the run and against open todos, plus a
  "merge duplicates" instruction in the prompt. Never remove any of these
  layers — the failure mode was duplicated calendar events/todos when manual
  enhance raced the invisible auto-generation.
- An `auto-organized` toast tells the user what was created.

## Todos (`TodosView.tsx`, `history.rs`)

Simple table (`todos`) with CRUD commands; manual add from the UI; a
convert-to-event affordance. Most todos arrive automatically from meetings or
voice commands.

## Ask your notes (`ask.rs`, `AskView.tsx`)

- Home's bottom bar takes a question → creates an **ask session** (row in
  `ask_sessions`), which appears under "SEARCHES" in the sidebar and routes to
  `AskView`.
- `answer()` runs keyword search over meeting notes
  (`history.search_meeting_notes`), assembles context from ≤6 meetings /
  ~24 k chars, asks the LLM to answer **citing meeting titles and dates**,
  emits `ask-answer`, and stores the answer on the session.
- Provider is selectable per ask (cloud vs local) via a `<select>` persisted
  in `localStorage["noted.askProvider"]`; `provider_id` override goes through
  `answer_ask_session`.
- Search is **keyword-based on purpose**: `query_keywords` drops stopwords,
  keeps terms of length ≥ 2, caps at 8; `match_score` ranks by keyword hits
  with a +100 whole-phrase bonus; `snippet_around` builds the source
  snippets. A whole-sentence LIKE match was tried first and found nothing —
  don't regress to it.
- Same single-flight pattern as notes (`IN_FLIGHT` on session id).

## Voice commands (`voice_commands.rs`)

Spoken through **ordinary dictation** (not meetings). If the final dictation
text starts with a trigger, it is executed instead of pasted:

- Event triggers ("add an event", "set an event", "schedule…", "add to my
  calendar", "put on my calendar"…) → calendar via the same `cal-add` bridge;
  a date that can't be parsed falls back to a todo so nothing is lost.
- Todo triggers ("add todo", "add a task", "remind me to", "set a
  reminder"…) → todo.
- Polite lead-ins are stripped repeatedly first ("can you", "could you",
  "please", "okay", "hey", "ok"), all matching is word-boundary-safe
  ("add todos…" is NOT a command), and event phrasing supports both
  date-last ("dentist **on friday at 3pm**") and date-first
  ("**on Sunday,** update the website") splits — see
  `event_split_candidates`.
- Outcome is toasted via the `voice-command` event
  (`todo | event | event_fallback_todo`).
- English-only triggers for now. Meetings do NOT use this parser — their
  action items come from the LLM pass above.

## Dictation post-processing

Separate from all the above: the `transcribe_with_post_process` shortcut runs
the dictation transcript through the user's "Improve Transcriptions" prompt
(cleanup, lists, self-correction removal). Meetings never paste and never use
this prompt.
