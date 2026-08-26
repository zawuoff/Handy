# Noted — agent onboarding docs

This folder documents **Noted**, a fork of [Handy](https://github.com/cjpais/Handy)
(upstream `cjpais/Handy`, this fork `zawuoff/Handy`). It exists so that any AI
agent (or human) can pick the project up cold and work on it safely.

Read these files in order the first time; after that, jump to whichever one
covers your task. **[gotchas.md](gotchas.md) is mandatory reading before any
code change** — it lists the mistakes that have already been made once.

| File                                       | Covers                                                                                  |
| ------------------------------------------ | --------------------------------------------------------------------------------------- |
| [architecture.md](architecture.md)         | What the app is, tech stack, module map, data flow                                      |
| [meetings.md](meetings.md)                 | Meeting mode end to end: session, live view, streaming, pause reset, speaker separation |
| [notes-and-ai.md](notes-and-ai.md)         | Note generation, prompts, todos/calendar extraction, ask sessions, voice commands       |
| [data-and-storage.md](data-and-storage.md) | SQLite schema, migration rules, settings store, on-disk layout                          |
| [integrations.md](integrations.md)         | MCP server for Claude/Codex, GNOME calendar, meeting radar                              |
| [dev-workflow.md](dev-workflow.md)         | Build/test/verify loop, release + install, bindings sync, i18n, git                     |
| [gotchas.md](gotchas.md)                   | Hard-won lessons; things that silently break if you don't know them                     |

## What Noted is

Noted is a **fully local** desktop app for Linux (the user runs Ubuntu with
GNOME on Wayland) that does two jobs:

1. **Dictation** — hold a global shortcut, speak, and the transcribed text is
   pasted into whatever app has focus. This is inherited from upstream Handy.
2. **Meeting notes (Granola-style)** — a separate shortcut/UI records a whole
   meeting from the microphone, shows a live transcript with speaker labels,
   lets the user jot notes during the meeting, and when it ends produces
   polished English notes via an LLM, extracts action items into todos and
   calendar events, and stores everything in a searchable local library.

**Privacy is the core promise.** Speech-to-text runs on-device with bundled
models (transcribe-cpp GGUF / ONNX). The only network use is (a) optional LLM
post-processing through user-configured providers — the user's default is a
"custom" OpenAI-compatible provider (model `deepseek-v4-flash`) — and
(b) model downloads. The UI repeats "Transcribed on this device. Nothing
leaves your computer."

## The user

- GitHub `zawuoff`, non-technical. Explain things in plain language, avoid
  jargon in user-facing chat, and never make them run terminal commands —
  run builds and installs for them (see [dev-workflow.md](dev-workflow.md)).
- Speaks English, Hindi, and Arabic in meetings — mixed-language handling
  matters (see the pause-reset section of [meetings.md](meetings.md)).
- Design decisions come from their Paper design file `noted_design`
  (file id `01M0XCZ8GJVKAVA77PZN8CRGFR`, 5 mockups + inspo board). The current
  palette is near-black surfaces with a soft blue accent — see
  `src/styles/theme.css`, which is the single source of truth for colors.

## Fork relationship

Upstream Handy is under a **feature freeze** and explicitly declined meeting
features, so this fork does **not** open PRs upstream; work is pushed straight
to `origin main` (`github.com/zawuoff/Handy`). Upstream's contributor rules in
`AGENTS.md` / `.github/` still apply if you ever do touch upstream.

Everything user-facing says "Noted"; the bundle identifier stays
**`com.pais.handy`** on purpose (see gotchas — do not change it), the binary
is still named `handy`, and the app data dir is `~/.local/share/com.pais.handy/`.
