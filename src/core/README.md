# core/

Reusable frontend plumbing every fork of this template builds on.
Everything under `@/core/*` is expected to stay shape-compatible as
the template evolves — fork-specific UI (upload buttons, chart
renderers, dashboards) lives under `src/features/<your-app>/`.

## Layout

```
core/
  apiAdapter.ts       REST + WebSocket + tool_result_from_ui + closeSession
  hooks/
    useClaudeSession.ts   stream-json → state, pending-tool queue, reset
                          (exports UseClaudeSessionReturn for lifting
                           the session up to a parent that needs
                           sessionId, e.g. for dataset binding)
    useTheme.ts           light/dark toggle, localStorage-backed
  components/
    ChatView.tsx          default bubble chat; see "slot props" below
    SessionRunner.tsx     lower-level primitive, no bubbles
    MessageList.tsx       raw stream-json dump
    PromptInput.tsx       textarea + Send/Cancel + leftAdornment slot
    PendingToolCalls.tsx  dock-style tool-call renderer
    Markdown.tsx          react-markdown + remark-gfm wrapper
    ThinkingBubble.tsx    bouncing-dots loader
    ThemeToggle.tsx       sun/moon icon button
  tools/
    registry.ts           clientToolRegistry + toolResultRegistry + types
    builtins/             empty in the viz template; forks place
                          reference renderers here (see src/features/
                          for the viz-specific ones this template ships)
```

## What each layer owns

- **`apiAdapter.ts`** — the single seam between frontend code and the
  backend. Components should never call `fetch(...)` or
  `new WebSocket(...)` directly. `apiCall(command, params)` is for
  streaming Claude commands; for domain REST endpoints (dataset upload
  etc.), add typed helpers under `src/features/<name>/api.ts`.
- **`useClaudeSession`** — the only intended way to drive a
  conversation. Exposes `send`, `cancel`, `reset`, `resolveToolCall`,
  `removePendingToolCall`, plus reactive `messages`, `status`, `error`,
  `pendingToolCalls`, and `sessionId`. Can be called inside
  `<ChatView>` or lifted to a parent that needs `sessionId` — see the
  `session` prop on `<ChatView>` below.
- **`<ChatView>`** — default UI most forks keep and splice into with
  slot props rather than rewriting. The viz app uses all four slots;
  domain-specific forks will too.
- **`tools/registry.ts`** — two small registries populated at app boot
  in `src/main.tsx`. `clientToolRegistry` (name → React component for
  interactive tools Claude calls) and `toolResultRegistry` (name →
  React component for rendering a tool's result bubble). Both are
  optional to extend; unregistered tools fall back to sensible defaults
  (nothing rendered for client tools without a registration; a
  collapsed JSON `<details>` preview for unknown tool results).

## `<ChatView>` slot props

The viz shell (`src/App.tsx`) uses these to layer dataset UI on top of
the generic chat without forking the component. Use the same pattern
for your own feature surfaces.

| Prop                      | Purpose                                                                 |
| ------------------------- | ----------------------------------------------------------------------- |
| `session`                 | Externally-owned `useClaudeSession()` return value. Lets the parent read `sessionId` to bind dataset state, etc. When omitted, `<ChatView>` creates its own session internally. |
| `headerExtra`             | Extra nodes beside the theme toggle in the header.                      |
| `composerLeftAdornment`   | Rendered inside `<PromptInput>` left of the textarea (file upload button, model picker, …). |
| `aboveComposer`           | Rendered between the message list and the composer (dataset chip, error banners, …). |
| `renderEmptyState`        | Override the default empty-state with fork-specific onboarding.         |
| `onBeforeSend(prompt)`    | Runs before `session.send`. Awaited — throw to abort. Use for side effects like binding a dataset to the session. |
| `onReset()`               | Runs **before** `session.reset()` so callers can observe the old `sessionId` (unbind server-side state, cancel pending work) before the hook rotates it. |

## `<PromptInput>` `leftAdornment`

Slot inside the composer for feature controls (file attach, slash-
command menu, model selector). Keeps `<PromptInput>` domain-agnostic
while letting features splice in UI.

## `<ChartResultCard>` (not in core/)

Lives at [src/features/viz/ChartResultCard.tsx](../features/viz/ChartResultCard.tsx).
Registered via `registerToolResult("create_chart", ChartResultCard)`
in [src/main.tsx](../main.tsx). Renders the Vega-Lite spec returned
by the Python `create_chart` tool using `react-vega`. Measures its
wrapper via `ResizeObserver` and passes a concrete numeric `width`
into the spec — Vega-Lite's `width: "container"` mode doesn't reliably
recover from an initial zero-width render.

## Import style

Import from deep paths today (`@/core/hooks/useClaudeSession`,
`@/core/tools/registry`, etc.) — there's no barrel `index.ts`. If your
fork wants one, add it here and keep the public surface narrow.

## Features vs core

Code that depends on a specific tool name, backend route, or
domain-shape schema goes in `src/features/<name>/`, **not** here.
Current viz-template features:

- [src/features/datasets/](../features/datasets/) — CSV/JSON upload
  button, chip, and REST client for `/api/datasets/{upload,bind,unbind}`.
- [src/features/viz/](../features/viz/) — `ChartResultCard` renderer
  for the Python `create_chart` tool.

Delete these when you fork and replace them with your domain's
feature modules (e.g. `features/finance/`, `features/ml-eval/`).
