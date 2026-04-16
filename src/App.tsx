import { ChatView } from "@/core/components/ChatView";

/**
 * Bare-template shell for customer-facing Claude-powered apps.
 *
 * Single-screen chat. No project picker, no server-side filesystem
 * exposure. Claude runs without a `cwd` and without its built-in tools
 * (no Read/Write/Bash) — domain behavior comes from tools the fork
 * registers in two places:
 *  - `backend/src/main.rs` via `ToolRegistry::builder().server_tool(…)`
 *    or `.client_tool(…)` for the tools Claude sees.
 *  - `src/main.tsx` via `registerClientTool(…)` for the React
 *    components that render client tools.
 *
 * Replace this file freely. The stable surface for bespoke UIs is
 * `@/core/*` — `useClaudeSession`, `<ChatView>`, `<SessionRunner>`,
 * `<MessageList>`, `<PromptInput>`, and the tool registries.
 *
 * The original opcode tab-based developer shell is archived at
 * `src/App.opcode-full.tsx.archive` if you want to port pieces back in.
 */
export default function App() {
  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <ChatView />
    </div>
  );
}
