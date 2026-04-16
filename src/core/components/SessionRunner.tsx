import { useClaudeSession } from "@/core/hooks/useClaudeSession";
import { MessageList } from "@/core/components/MessageList";
import { PromptInput } from "@/core/components/PromptInput";
import { PendingToolCalls } from "@/core/components/PendingToolCalls";

/**
 * Reference composition: wires `useClaudeSession` to a message list + prompt
 * input. For customer-facing apps no `projectPath` is passed — Claude runs
 * without a cwd and with all built-in tools disabled (see
 * `resolve_default_tools` in the backend). Forks building dev tools can
 * still pass one.
 *
 * This is the 90%-case starting point. Phase 5 replaces it with a proper
 * `ChatView` that renders tool calls as domain components.
 */
export function SessionRunner({
  projectPath,
  model,
}: {
  projectPath?: string;
  model?: string;
}) {
  const session = useClaudeSession({ projectPath, model });
  const running = session.status === "running";

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b p-3">
        <div className="font-mono text-xs text-muted-foreground">
          session {session.sessionId.slice(0, 8)} · {session.status}
        </div>
      </header>

      <MessageList messages={session.messages} />

      <PendingToolCalls
        calls={session.pendingToolCalls}
        onResolve={session.resolveToolCall}
      />

      {session.error && (
        <div className="border-t bg-destructive/10 p-2 text-xs text-destructive">
          {session.error}
        </div>
      )}

      <PromptInput
        onSubmit={session.send}
        onCancel={session.cancel}
        disabled={running}
        running={running}
      />
    </div>
  );
}
