# Jarvis Architecture

## System Shape

Jarvis is a Rust orchestrator with two Python inference workers and an
optional local React configuration page.

```text
Rust main.rs
  -> configuration and startup checks
  -> Orchestrator
       -> STT Python worker
       -> TTS Python worker
       -> LLM provider
       -> ToolRegistry
       -> audio playback and conversation pipeline

Rust config_ui (Axum, 127.0.0.1)
  -> config.yaml API
  -> web/config-ui/dist static files
```

## Rust Responsibilities

`src/main.rs` parses CLI options, loads configuration, initializes logging,
starts the local configuration server, and selects normal, text, or
configuration-only execution.

`src/orchestrator.rs` owns the conversation state machine. It coordinates wake
gating, STT events, LLM turns, TTS playback, confirmations, barge-in, silence
mode, reminders, and graceful worker recovery.

[`src/config.rs`](../src/config.rs) defines the serialized configuration and
its defaults. Missing YAML keys are completed through serde defaults. The
checked-in [`config.example.yaml`](../config.example.yaml) is only a portable
starting point; `config.yaml` is a local ignored file.

`src/tools/` contains built-in and user-defined tools. Each tool exposes its
schema, execution behavior, spoken description, and deterministic risk level.

[`src/config_ui/`](../src/config_ui) serves the local configuration frontend
and provides REST and WebSocket endpoints for configuration and microphone
calibration. It does not
share mutable configuration state with a running orchestrator; requests read
and write the YAML file directly. Saved changes require a restart.

## Python Workers

`workers/stt_worker.py` owns microphone capture, voice activity detection,
Whisper transcription, clap detection, calibration, and optional speaker
verification.

`workers/tts_worker.py` owns local Piper synthesis when the Piper provider is
selected.

Workers are spawned by Rust through `WorkerHandle`. Messages use newline
delimited JSON on stdin/stdout. TTS may append raw audio bytes after a message
header. Logs from Python libraries must remain on stderr so stdout stays a
valid protocol stream.

`JARVIS_RUNTIME_DIR` is injected by Rust and is the shared location for worker
cache and runtime artifacts. See [`src/ipc/process.rs`](../src/ipc/process.rs)
for the process boundary. Workers must not write user data into the source tree
outside explicitly ignored runtime paths.

## Conversation Flow

1. STT emits a transcript or a control event.
2. The wake gate decides whether the transcript is directed at Jarvis.
3. The orchestrator builds the current system context, including recent memory.
4. The LLM streams text and may request tools.
5. Safe tools run immediately; Confirm and Code tools enter the Rust-owned
   confirmation flow.
6. Text is split, sanitized, synthesized, and played while the LLM may still
   be streaming.
7. The orchestrator returns to listening, remains silent when requested, or
   resumes the pending confirmation/turn state.

## Configuration UI Flow

The frontend is built with Vite and React, then served by the Rust binary from
[`web/config-ui/`](../web/config-ui) and its generated `dist/`. In standalone
mode, `jarvis --config-ui` starts only the
local server and may open the browser. In normal mode, `web_ui.enabled` starts
the server as a background task without opening a browser.

The API is intentionally localhost-only. Configuration writes serialize the
whole in-memory configuration and create a `.bak` backup before the first
write. Comments in the YAML are not preserved by a UI save.

## Platform Boundaries

The project is designed for Windows, Linux, and macOS, but capabilities vary:

- Windows-specific audio, process, and application discovery code is guarded
  with `cfg(windows)`.
- Unix builds use the Unix platform module and shell tool.
- Audio drivers, Wayland/X11 input control, microphone access, and provider
  availability require platform-specific verification.
- CI currently builds and tests Rust on Windows and Ubuntu. It does not run
  the full STT/TTS application with real hardware or providers.

## Removed Surface

The terminal Ratatui interface was removed. There is no terminal visual state,
terminal renderer, TUI configuration, or TUI-specific logging path. The
supported interaction surfaces are voice, text mode, and the local web
configuration page.
