# Contributing To Jarvis

## Before A Change

Read [`AGENTS.md`](../AGENTS.md), the relevant
[`ARCHITECTURE.md`](ARCHITECTURE.md), the current source, and the existing
tests. Confirm whether the request changes product behavior,
security, persistence, protocol, or platform support.

Ask before choosing between competing product, UX, architecture, security, or
privacy alternatives. Prefer a small change over a broad refactor.

## Repository Boundaries

- Rust owns orchestration, policy, providers, tools, and risk decisions.
- Python workers own ML inference and audio capture through the existing IPC.
- React owns the local configuration UI, not core Jarvis behavior.
- `config.example.yaml` is versioned; `config.yaml` is personal and ignored.
  Neither file carries comments: configuration is documented only in
  [`CONFIGURACION.md`](CONFIGURACION.md), because a UI save strips comments
  and the example is copied verbatim into the personal file.
- Runtime files, secrets, models, voices, recordings, databases, and logs are
  never committed.
- The removed terminal TUI must not be reintroduced.

## Feature Proposal

For a non-trivial feature, record the current extension points, alternatives,
trade-offs, complexity, iteration/recursion or concurrency needs, platform
impact, security impact, and acceptance tests before implementation.

Use independent reviewers for complex work. A context-aware reviewer checks
architectural fit, a clean-context reviewer challenges assumptions, and a
security reviewer checks tools, network, permissions, and private data when
applicable.

## Code Comments

Comments explain why, not what. Avoid comments that restate names or syntax.
Use them for non-obvious decisions, trade-offs, workarounds, side effects,
ordering constraints, external bugs, and rules that code alone cannot express.

## Verification Commands

```bash
cargo fmt --check
cargo build
cargo test
python -m pytest workers/tests -v
cd web/config-ui && npm run build
cd web/config-ui && npm run lint
```

The full STT/TTS runtime needs audio hardware, Python ML dependencies, model
files, and an LLM/TTS provider. CI therefore tests Rust, pure Python logic,
frontend build/lint when run locally, and the privacy guard rather than the
complete voice loop.

## Documentation Updates

Update the relevant canonical document when behavior changes:

- Product intent and status: [`PRODUCT.md`](PRODUCT.md) and
  [`ROADMAP.md`](ROADMAP.md).
- Visual and interaction rules: [`DESIGN.md`](DESIGN.md).
- Technical structure: [`ARCHITECTURE.md`](ARCHITECTURE.md).
- Security and privacy: [`SECURITY.md`](SECURITY.md).
- Configuration reference: [`CONFIGURACION.md`](CONFIGURACION.md).
- User setup and usage: [`README.md`](../README.md).
