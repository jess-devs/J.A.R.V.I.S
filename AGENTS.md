# Jarvis Agent Instructions

## Project Intent

Jarvis is a Spanish-first, local-first voice assistant that runs as a
background process. It listens for an explicit activation, helps the user with
conversation and computer tasks, can be silenced, and is intended to support
natural interruption and turn-taking over time.

The current repository is optimized for the developer's real machine, but new
designs must not hard-code personal paths, devices, providers, aliases, or
preferences. The architecture is intended to remain portable across Windows,
Linux, and macOS.

## Read Before Editing

1. Read the relevant source, tests, configuration types, and documentation
   before proposing a change.
2. Identify the existing extension point before adding a new abstraction.
3. Check the current git diff and preserve unrelated user changes.
4. Treat `src/config.rs` as the source of default values. Treat
   `config.example.yaml` as the portable, versioned example. Never add the
   personal `config.yaml` to git.
5. Never write comments into `config.yaml` or `config.example.yaml`.
   [`docs/CONFIGURACION.md`](docs/CONFIGURACION.md) is the only place where
   configuration is documented. A UI save serializes the whole file and drops
   every comment, and the example is copied verbatim into the personal file,
   so a YAML comment is both short-lived and duplicated. Explain a key in its
   section table instead.
6. Keep `web_ui` (the local React/Axum configuration page) distinct from any
   removed terminal UI. The terminal TUI is intentionally deleted and must
   not be reintroduced.

## Architecture Boundaries

- Rust (`src/`) owns orchestration, configuration, networking, risk decisions,
  tool registration, audio playback, and the conversation pipeline.
- Python (`workers/`) owns ML inference and microphone capture. Workers are
  child processes and communicate with Rust through the NDJSON stdio protocol.
- React/Vite (`web/config-ui/`) is an optional local configuration frontend.
  Rust serves its built static files and exposes the localhost-only API.
- Runtime data belongs under `runtime.dir` and must never be committed.
- Worker stdout is a protocol stream. Worker diagnostics belong on stderr.
- Changes to the worker protocol must be additive unless the user explicitly
  approves a breaking migration.

## Non-Negotiable Safety Rules

- Never commit `.env`, API keys, databases, logs, recordings, transcripts,
  speaker embeddings, downloaded voices, models, or personal paths.
- Do not add telemetry or remote data collection by default.
- Cloud providers must remain explicit user configuration, never an implicit
  fallback from the local mode.
- Risk classification is deterministic Rust code, never an LLM decision.
- `Safe`, `Confirm`, and `Code` behavior must remain explicit and testable.
- Never weaken shell, file, network, MCP, confirmation, or speaker-verification
  safeguards without explicit approval and focused tests.
- Keep the configuration API bound to `127.0.0.1` unless the user explicitly
  requests a reviewed network-access design.
- Jarvis must not execute commands, modify/delete data, transmit data, or
  activate sensors without an explicit valid user request and the applicable
  safety gate.

## Product And UX Decisions

Ask the user before deciding any of the following:

- Product scope, user-visible behavior, naming, interaction flows, or visual
  direction.
- New dependencies, processes, protocols, persistence formats, or platform
  assumptions.
- Security, privacy, network, permissions, telemetry, or destructive behavior.
- A change that is costly to reverse or changes an external contract.

Do not silently choose between competing product alternatives. Present the
options, trade-offs, risks, and a recommendation, then wait for approval.

## Feature Analysis Gate

Before implementing a non-trivial feature, document or report:

- Existing code paths and the smallest suitable extension point.
- At least two implementation alternatives.
- Advantages, disadvantages, migration impact, and failure modes.
- Whether iteration, recursion, queues, polling, or concurrency are needed.
- Time and space complexity where it matters.
- Platform, privacy, security, and operational effects.
- Tests and manual verification needed for acceptance.

For complex work, use independent review when useful: one reviewer with
repository context, one reviewer with a clean context to challenge assumptions,
and a security reviewer for tools, network, permissions, or private data.
Reviewers provide input; they do not make product decisions for the user.

## Code Style

Comments explain why, not what. Do not add comments that repeat a function,
variable, or line of code. Use comments for non-obvious decisions, trade-offs,
temporary workarounds, side effects, ordering constraints, external bugs, or
business rules that the code cannot express.

Prefer the smallest correct change. Avoid speculative compatibility layers,
unrelated refactors, and new abstractions without a concrete reuse case.

## Verification

Run the narrowest relevant checks while iterating and the full checks before
completion:

```bash
cargo fmt --check
cargo build
cargo test
python -m pytest workers/tests -v
cd web/config-ui && npm run build
cd web/config-ui && npm run lint
```

If a check cannot run because it requires hardware, a provider, or an
operating-system-specific environment, report that limitation and perform the
strongest available substitute.

## Documentation Contract

Update documentation when behavior, configuration, security, platform support,
or user-visible UX changes. Keep product intent in
[`docs/PRODUCT.md`](docs/PRODUCT.md), visual rules in
[`docs/DESIGN.md`](docs/DESIGN.md), technical structure in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), and security constraints in
[`docs/SECURITY.md`](docs/SECURITY.md). Contribution workflow lives in
[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md).
