# Gotchas — read before changing anything

Every item here caused a real bug, a wasted build, or user-visible breakage
at least once. The details live in git history; the rules live here.

## Data & identity

1. **SQLite migrations are positional — append only.** New migrations go at
   the END of the `MIGRATIONS` list in `managers/history.rs`. Inserting one
   mid-list makes upgraded databases silently skip it (rusqlite_migration
   tracks progress by index via `user_version`). This happened once; it was
   caught only by manually inspecting the user's live DB before launch.
2. **Never change the bundle identifier `com.pais.handy`.** All user data
   (history.db, settings, models, recordings) lives under
   `~/.local/share/com.pais.handy/`. Rebranding stops at strings, icons, and
   bundle names (`Noted_0.9.6_amd64.deb`); the identifier and the `handy`
   binary name stay.
3. **Changing a settings default ≠ changing users' settings.** Stored
   settings keep the old default forever. To upgrade an unchanged default
   (e.g. the meeting-notes prompt), add a one-time migration in `settings.rs`
   that recognizes the _exact old default text_ and replaces it — an edited
   value is the user's and must survive.

## Concurrency & duplication

4. **Every background LLM job needs a single-flight guard.** Cold local
   models take 40–75 s; users click again. The combination that works:
   in-process `IN_FLIGHT` set + atomic DB claim
   (`UPDATE … WHERE flag = 0`, require `rows == 1`) + idempotent output
   (dedup by normalized title). The duplicate-todos/events incident came from
   missing exactly this.
5. **Meeting-end data handoff** must go through the `PENDING_*` statics in
   `meeting.rs` (live notes, speaker names). The session is torn down at stop
   but the save pipeline runs later and async — reading session state there
   returns nothing.

## Streaming / models

6. **Sortformer (speaker diarizer) has NO stream API** in transcribe-cpp
   0.2.0 — `stream begin` returns "not implemented" even though its GGUF
   metadata says `streaming=true`. It must be driven with `session.run()`
   over accumulated audio (see `diarization.rs::Diarizer`). Don't "simplify"
   it back to a stream.
7. **Small streaming ASR models lock into a language/script** because they
   condition on their own committed output. The pause-reset in
   `run_stream_worker` (finalize + fresh stream after ~1.5 s silence) is the
   fix — removing it re-breaks the user's Hindi/English/Arabic meetings.
   Conversely, the **diarizer must NOT be restarted** at those resets or
   speaker ids renumber; only the ASR stream restarts.
8. **Time bases differ**: ASR segment times are local to the current stream
   segment (reset at every pause reset); the diarizer clock is
   meeting-global (`total_fed_ms`). Always map with `offset_ms` (the
   `AsrRecord` fields) before comparing.
9. **The stream finalize handshake has a 30 s budget**
   (`STREAM_FINALIZE_REPLY_TIMEOUT`). Anything that runs inside finalize
   (e.g. the diarizer's last pass, capped at 10 s) must fit inside it or the
   caller times out and falls back to batch transcription.
10. **Nemotron streaming returns empty for clips < ~2.5 s.** That was once
    misdiagnosed as "dictation is broken". Short dictations need Parakeet;
    verify model behavior with `handy --transcribe-file <wav>` before blaming
    code.
11. **Silence detection feeds the pause reset** — cheap RMS on fed frames.
    If VAD/recording changes ever stop silent frames from reaching the
    stream worker, the pause reset (and diarizer refresh cadence) breaks.

## Platform (GNOME Wayland)

12. **Apps cannot position their own windows on Wayland.** The overlay/pill
    positioning fix forces `GDK_BACKEND=x11` (XWayland) on GNOME Wayland in
    `main.rs`, **unconditionally** — the user's session exports
    `GDK_BACKEND=wayland` globally, so "respect an existing value" logic
    silently failed. Opt-out: `HANDY_NO_X11_FALLBACK=1`.
13. **Tray**: SNI/dbusmenu cannot host widgets; menu _labels_ can update live
    and `TrayIcon::set_title` shows text next to the icon on GNOME (Ubuntu
    ships the AppIndicator extension) but not on KDE/Windows. The tray is a
    single-writer design in `tray.rs` — route all updates through it.
14. **Stale desktop icons**: old `~/.local/share/icons/**/handy.png` and
    `~/.local/share/applications/handy.desktop` from an old AppImage can
    shadow the system icon after install. Delete + `gtk-update-icon-cache`
    if the launcher shows the wrong icon.
15. **No passwordless sudo.** Installs go through
    `DISPLAY=:0 pkexec apt-get install --reinstall -y <deb>` which pops a GUI
    password dialog — warn the user it's coming. `sudo -n` fails.

## Frontend

16. **`bindings.ts` is hand-maintained between dev runs** — see
    [dev-workflow.md](dev-workflow.md#bindings). Forgetting the frontend half
    of a new command/event type is the most common way to break `tsc`.
    After a real specta regeneration, watch for duplicated entries.
17. **Every JSX string must be an i18n key in all 24 locales** — eslint +
    `check-translations.ts` enforce. Bulk-edit locales with a python script;
    beware escaping when generating Rust/JSON from python (raw strings).
18. **TipTap is v3** (`@tiptap/react@^3` + `tiptap-markdown@^0.9`; v2 has a
    peer conflict with tiptap-markdown). tiptap-markdown ships v2-only type
    augmentations, so `editor.storage.markdown` needs the typed cast helper
    in `RichNoteEditor.tsx`. The editor's external-content follow must
    compare against `doc` (draft included) and skip while focused, or it
    clobbers typing.
19. **`ToggleSwitch` renders its own `SettingContainer`** — pass
    `label`/`description`; don't wrap it in another container.
20. **Theme**: all palette colors are light/dark token pairs in
    `src/styles/theme.css` — never hard-code a palette hex in components.
    Current design: near-black + soft blue accent (`#72AEEC` dark accent),
    from the Paper file; the recording dot is intentionally blue, not red.

## Process

21. **Tests with ties/randomness flake**: a HashMap-ordering tie in
    `dominant_speaker` made a test fail intermittently. Deterministic
    tie-breaks (and re-running the suite a few times after adding tests) are
    cheap insurance.
22. **cargo fmt immediately** after generating Rust from scripts — later
    edits anchor on formatted text, and stale anchors are the #1 cause of
    failed scripted patches. When a python patch asserts an anchor, re-read
    the file if it fails; don't force it.
23. **Meetings are never lost silently**: the save path has a clipboard
    rescue (`meeting-save-failed` event copies the transcript). Preserve
    this invariant in any refactor of `actions.rs`.
24. **The user's `~/.local/bin/cal-add` and AFFiNE installation are theirs**
    — read cal-add, never modify it; never touch AFFiNE (an earlier request
    to delete it was explicitly retracted).
