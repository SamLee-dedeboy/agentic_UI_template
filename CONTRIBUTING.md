# Contributing

Thanks for considering a contribution to the claude-ui template.

Before starting, scan existing issues and PRs so you don't duplicate
in-flight work.

## Workflow

1. Fork the repo.
2. Branch off `main`.
3. Make your changes. Keep them tightly scoped — one concern per PR.
4. Ensure `bun run check` passes (typecheck + `cargo check`) and any
   tests relevant to the area you touched still pass (`cargo test`).
5. Open a PR with a clear description of the problem, the approach, and
   any caveats.

## PR guidelines

1. **Title prefix** — one of:
   - `Feature:` for new features
   - `Fix:` for bug fixes
   - `Docs:` for docs-only changes
   - `Refactor:` for code reorganization
   - `Improve:` for perf or UX polish
   - `Other:` for everything else

   Examples: `Feature: add seat-map client tool`, `Fix: reset doesn't cancel running session`.

2. **Description** — state what's broken (or missing), what you
   changed, and anything a reviewer should watch for.

3. **Docs** — if behavior changes, update the affected READMEs
   ([main](README.md), [docs/README.md](docs/README.md),
   [docs/tools.md](docs/tools.md), [CLAUDE.md](CLAUDE.md),
   [src/core/README.md](src/core/README.md)) in the same PR.

4. **Dependencies** — justify any new package; prefer stdlib / existing
   deps where possible. Track new deps in `package.json` /
   `backend/Cargo.toml`.

## Local testing

Commands you'll use most:

```bash
bun run check              # typecheck + cargo check
bun run build              # production frontend build
cd backend && cargo test   # Rust unit tests (core + examples)
cd backend && cargo fmt    # before committing Rust
```

Manual UI smoke: after any change, run the backend (`cargo run --bin
claude-ui-app`), open http://localhost:1420 in dev mode (`bun run dev`)
or :8080 in prod, and click through the three example prompts. Client
tool round-trips (the color picker, the flight picker) are the most
fragile path — always hit them end-to-end.

> **After editing backend Rust code**: stop the running backend (Ctrl+C),
> `cargo build --bins`, restart, and click "New chat" in the UI. The
> running process keeps its binary mmapped and won't pick up rebuilds;
> old Claude context inside an ongoing chat caches tool results and
> masks behavior changes.

## Coding standards

### Frontend (TypeScript / React 19)

- TypeScript for all new code; no `any` without a justifying comment.
- Functional components + hooks. No class components.
- Tailwind for styling, using existing design tokens (`bg-card`,
  `text-muted-foreground`, etc.) — don't introduce new color values
  unless the theme file is updated alongside.
- Build against `@/core/*` primitives (`useClaudeSession`, `<ChatView>`,
  `<Markdown>`, the tool registries) rather than calling `apiAdapter`
  directly from components or re-implementing tool-call routing.
- JSDoc comments for exported functions and components when the WHY
  isn't obvious from the code.

### Backend (Rust)

- Follow `rustfmt` defaults; run `cargo fmt` before committing.
- Handle all `Result` types explicitly — no silent `.unwrap()` in
  production paths.
- Add `///` docs for public items in `core::tools`, `web_server`, and
  anything on the fork-visible API surface.
- New server tools go in `backend/src/examples/` (or the fork's own
  module) and are wired through `backend/src/main.rs`.

### Security

- Validate all inputs that cross an external boundary (HTTP, WS, MCP).
- Use parameterized queries for SQLite — never string-interpolate SQL.
- Never log secrets (`APP_SESSION_KEY`, bearer tokens, bridge secrets,
  etc.).
- Preserve the customer-app defaults: `--tools ""`, no
  `--dangerously-skip-permissions` unless `APP_ALLOW_SKIP_PERMISSIONS=1`.

## Testing

- Add unit tests for new Rust helpers — examples in
  `backend/src/examples/flights.rs` and `backend/src/core/tools.rs`.
- For behavior that spans frontend + backend (a new tool), add at least
  a Rust-side dispatch test; frontend components can be exercised
  manually through the chat UI.
- Ensure existing tests pass (`cargo test`, `bun run check`).
