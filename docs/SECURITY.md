# Jarvis Security And Privacy

## Trust Model

Jarvis runs on the user's computer and can control parts of that computer. The
LLM is an untrusted planner: it may request a tool, but it never decides the
tool's risk level or confirms its own action.

Rust owns the final execution and confirmation decisions. See the
[`RiskLevel`](../src/tools/mod.rs) implementation and
[`confirm.rs`](../src/agent/confirm.rs). Python workers are child processes for
ML and audio work; their protocol and runtime boundaries must remain explicit.

## Tool Risk Levels

- `Safe` is read-only or low-impact and can execute directly.
- `Confirm` changes the system and requires an explicit user confirmation,
  unless the user deliberately selects the configured free mode.
- `Code` represents extreme risk and always requires the acceptance code.

Risk is assessed from the tool and its arguments in Rust. The acceptance code
is checked in Rust and is never sent to the LLM.

## Network Rules

- The configuration server listens on `127.0.0.1` only.
- The configuration API also validates the `Origin` header and rejects any
  request that does not come from the configuration page itself. Binding to
  loopback is not enough on its own: a WebSocket is not subject to the
  same-origin policy, so without this check any website the user visits could
  open the calibration socket and reach the microphone and the speaker
  embedding. A missing `Origin` is accepted, because browsers always send it
  for WebSocket handshakes and cross-origin requests. See `origin_allowed` in
  [`config_ui/mod.rs`](../src/config_ui/mod.rs).
- Cloud LLM and TTS providers are opt-in configuration, not implicit fallback.
- Web tools resolve hosts through [`net_guard.rs`](../src/net_guard.rs) before
  fetching and reject loopback, private,
  link-local, unspecified, and equivalent mapped addresses by default. The
  check does not close the window between resolution and fetch: `reqwest`
  resolves again, so a short-TTL rebinding attack is still possible.
- Scripted HTTP tools have their own timeout, host, and private-network guards.
- MCP servers are local child processes over stdio today. Untrusted MCP tools
  require confirmation by default.

Any change that broadens network access, adds a transport, or changes host
validation requires focused security review and tests.

## Local Data

Runtime data may include memory, reminders, scripted tools, audit events,
calibration caches, and speaker embeddings. These files are machine- and
user-specific and must remain under ignored runtime paths.

The repository must never contain API keys, `.env`, recordings, transcripts,
databases, logs, embeddings, downloaded models, downloaded voices, or personal
paths.

Jarvis has no telemetry requirement. New remote reporting or analytics requires
explicit product and privacy approval.

## Audio And Speaker Verification

Microphone capture is an explicit product capability and must respect the wake,
silence, and lifecycle states. Speaker verification is experimental and must
not be described as a guaranteed security boundary. Thresholds are empirical,
hardware-dependent, and must fail with an understandable user-facing result.

## Configuration Safety

[`config.example.yaml`](../config.example.yaml) is safe to commit. `config.yaml` is a local ignored
configuration and may contain paths, devices, provider choices, aliases, and
other private preferences. Never use the personal file as documentation or as
a fixture.

## Review Requirements

Security review is required for changes involving shell execution, file access,
mouse or screen control, network fetching, MCP, persistence, microphone data,
speaker embeddings, API keys, confirmation logic, or process isolation.
