# core/

Reusable plumbing every fork of this template builds on. This directory
is the stable surface for bespoke UIs — everything under `@/core/*` is
expected to stay shape-compatible as the template evolves.

**Do not put feature-specific code here.** Domain UI (flight pickers,
seat maps, payment forms, analytics dashboards) lives under
`src/features/<your-app>/` or in the fork's own module tree.

## Layout

```
core/
  apiAdapter.ts       REST + WebSocket + tool_result_from_ui + closeSession
  hooks/
    useClaudeSession.ts   stream-json → state, pending-tool queue, reset
    useTheme.ts           light/dark toggle, localStorage-backed
  components/
    ChatView.tsx          default customer-facing chat (bubbles + markdown)
    SessionRunner.tsx     lower-level primitive, no bubbles
    MessageList.tsx       raw stream-json dump
    PromptInput.tsx       textarea + Send/Cancel
    PendingToolCalls.tsx  dock-style tool-call renderer
    Markdown.tsx          react-markdown + remark-gfm wrapper
    ThinkingBubble.tsx    bouncing-dots loader
    ThemeToggle.tsx       sun/moon icon button
  tools/
    registry.ts           clientToolRegistry + toolResultRegistry + types
    builtins/             reference components (see below)
```

## What each layer owns

- **`apiAdapter.ts`** is the single seam between frontend code and the
  backend. Components should never call `fetch(...)` or
  `new WebSocket(...)` directly — route through `apiCall(command, params)`,
  `resolveClientToolCall(...)`, and `closeSession(...)`.
- **`useClaudeSession`** is the only intended way to drive a conversation.
  It exposes `send`, `cancel`, `reset`, `resolveToolCall`,
  `removePendingToolCall`, plus the reactive `messages`, `status`,
  `error`, `pendingToolCalls`, and `sessionId`.
- **`<ChatView>`** is the default UI most forks keep or lightly restyle.
  For non-bubble layouts, compose `<SessionRunner>` or assemble
  `MessageList` + `PromptInput` + `PendingToolCalls` yourself.
- **`tools/registry.ts`** carries two small registries populated at app
  boot in `src/main.tsx`: `clientToolRegistry` (name → React component
  handler for a client tool) and `toolResultRegistry` (name → React
  component that renders a tool's result bubble). Both are optional to
  extend — unregistered tools fall back to sensible defaults.

## Builtins

`tools/builtins/` ships the reference components that pair with the
three example tools in `backend/src/main.rs`:

| File                          | Role                                             |
| ----------------------------- | ------------------------------------------------ |
| `ShowChoice.tsx`              | Client-tool renderer for `show_choice`           |
| `FlightResults.tsx`           | Client-tool renderer for `show_flight_options`   |
| `WeatherResultCard.tsx`       | Result renderer for `get_weather`                |
| `ChoiceResultCard.tsx`        | Result renderer for `show_choice`                |
| `FlightPickResultCard.tsx`    | Result renderer for `show_flight_options`        |

Forks typically delete these once they've wired their own tools.

## Import style

Import from deep paths today (`@/core/hooks/useClaudeSession`,
`@/core/tools/registry`, etc.) — there's no barrel `index.ts`. If your
fork wants one, add it at the top of this directory and keep the public
surface narrow.
