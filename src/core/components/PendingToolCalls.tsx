import type { PendingToolCall } from "@/core/hooks/useClaudeSession";
import { clientToolRegistry } from "@/core/tools/registry";

/**
 * Renders any client-tool calls the backend is currently blocked on.
 * Each pending call is looked up in the fork-populated
 * `clientToolRegistry`. Unknown names surface a warning card so forks
 * notice missing registrations instead of Claude hanging forever (the
 * backend has a 120 s timeout regardless).
 *
 * This is intentionally a thin coordinator — the actual per-tool UI
 * lives in the registered components, so forks swap visuals by swapping
 * registry entries, not by editing this file.
 */
export function PendingToolCalls({
  calls,
  onResolve,
}: {
  calls: PendingToolCall[];
  onResolve: (toolCallId: string, content: unknown) => void;
}) {
  if (calls.length === 0) return null;
  return (
    <div className="border-t bg-muted/30 p-3 space-y-2">
      {calls.map((call) => {
        const Component = clientToolRegistry[call.name];
        if (!Component) {
          return (
            <div
              key={call.tool_call_id}
              className="rounded border border-destructive/40 bg-destructive/10 p-2 text-xs text-destructive"
            >
              No client-tool component registered for <code>{call.name}</code>.
              Add one via <code>registerClientTool</code> in your app entry.
            </div>
          );
        }
        return (
          <Component
            key={call.tool_call_id}
            input={call.input}
            resolve={(content) => onResolve(call.tool_call_id, content)}
          />
        );
      })}
    </div>
  );
}
