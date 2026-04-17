import { useEffect, useMemo, useRef, type ReactNode } from "react";
import {
  useClaudeSession,
  type ClaudeMessage,
  type PendingToolCall,
  type UseClaudeSessionReturn,
} from "@/core/hooks/useClaudeSession";
import { PromptInput } from "@/core/components/PromptInput";
import { ThemeToggle } from "@/core/components/ThemeToggle";
import { ThinkingBubble } from "@/core/components/ThinkingBubble";
import { Markdown } from "@/core/components/Markdown";
import {
  clientToolRegistry,
  toolResultRegistry,
} from "@/core/tools/registry";
import { Button } from "@/components/ui/button";

/**
 * Default customer-facing chat view. Renders turns as bubbles, collapses
 * thinking blocks, and inlines client-tool calls as live components —
 * paired with the originating `tool_use` block via (name, input) match
 * so the picker renders in conversation flow, not as a dock.
 *
 * Forks can either pass their own `session` (owned by a parent that
 * needs to layer in dataset state, upload buttons, etc.) or let the
 * view create one internally. The optional `composerLeftAdornment`,
 * `aboveComposer`, `renderEmptyState`, `onBeforeSend`, and `onReset`
 * hooks let a parent splice in extra UI + behavior without forking
 * this file.
 */
export interface ChatViewProps {
  model?: string;
  /** Externally-owned session. When omitted, the view creates one. */
  session?: UseClaudeSessionReturn;
  /** Extra nodes rendered next to the theme toggle in the header. */
  headerExtra?: ReactNode;
  /** Rendered inside the PromptInput, left of the textarea. */
  composerLeftAdornment?: ReactNode;
  /** Rendered between the messages list and the composer. */
  aboveComposer?: ReactNode;
  /** Override the default empty-state content. */
  renderEmptyState?: (ctx: { onPick: (prompt: string) => void }) => ReactNode;
  /**
   * Called before `session.send`. Useful for side effects that must
   * complete first (e.g. bind a dataset to the session). Throwing
   * aborts the send.
   */
  onBeforeSend?: (prompt: string) => Promise<void> | void;
  /** Called after `session.reset`. */
  onReset?: () => void;
}

export function ChatView(props: ChatViewProps = {}) {
  // Split: when the caller owns the session, skip creating a second one.
  // When they don't, create one in a child component so we still obey
  // the Rules of Hooks (the hook call is unconditional inside the
  // branch that runs it).
  if (props.session) {
    return <ChatViewInner {...props} session={props.session} />;
  }
  return <ChatViewSelfOwned {...props} />;
}

function ChatViewSelfOwned(props: ChatViewProps) {
  const session = useClaudeSession({ model: props.model });
  return <ChatViewInner {...props} session={session} />;
}

function ChatViewInner({
  session,
  headerExtra,
  composerLeftAdornment,
  aboveComposer,
  renderEmptyState,
  onBeforeSend,
  onReset,
}: ChatViewProps & { session: UseClaudeSessionReturn }) {
  const running = session.status === "running";

  const turns = useMemo(() => projectTurns(session.messages), [session.messages]);

  // `tool_use.id` → bare tool name. Used by `tool_result` bubbles to
  // look up a custom result renderer, since those blocks only carry
  // `tool_use_id`.
  const toolNameById = useMemo(() => {
    const m = new Map<string, string>();
    for (const t of turns) {
      if (t.role !== "assistant") continue;
      for (const b of t.blocks) {
        if (b.kind === "tool_use") m.set(b.id, stripMcpPrefix(b.name));
      }
    }
    return m;
  }, [turns]);

  // Set of `tool_use.id`s that already have a `tool_result`.
  const resolvedToolUseIds = useMemo(() => {
    const s = new Set<string>();
    for (const t of turns) {
      if (t.role !== "assistant") continue;
      for (const b of t.blocks) {
        if (b.kind === "tool_result") s.add(b.tool_use_id);
      }
    }
    return s;
  }, [turns]);

  // Correlate each pending client-tool call to a `tool_use` block by
  // matching tool name + JSON-equal input.
  const pendingByToolUseId = useMemo(() => {
    const out = new Map<string, PendingToolCall>();
    const used = new Set<string>();
    for (const t of turns) {
      if (t.role !== "assistant") continue;
      for (const b of t.blocks) {
        if (b.kind !== "tool_use") continue;
        if (resolvedToolUseIds.has(b.id)) continue;
        const name = stripMcpPrefix(b.name);
        const match = session.pendingToolCalls.find(
          (p) =>
            !used.has(p.tool_call_id) &&
            p.name === name &&
            stableStringify(p.input) === stableStringify(b.input),
        );
        if (match) {
          used.add(match.tool_call_id);
          out.set(b.id, match);
        }
      }
    }
    return out;
  }, [turns, session.pendingToolCalls, resolvedToolUseIds]);

  // Safety net: drop pending client-tool calls whose `tool_result` has
  // already arrived from Claude.
  useEffect(() => {
    const toRemove: string[] = [];
    for (const p of session.pendingToolCalls) {
      const pairedToolUseId = [...pendingByToolUseId.entries()].find(
        ([, pp]) => pp.tool_call_id === p.tool_call_id,
      )?.[0];
      if (pairedToolUseId && resolvedToolUseIds.has(pairedToolUseId)) {
        toRemove.push(p.tool_call_id);
      }
    }
    for (const id of toRemove) session.removePendingToolCall(id);
  }, [
    pendingByToolUseId,
    resolvedToolUseIds,
    session.pendingToolCalls,
    session.removePendingToolCall,
  ]);

  const endRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [turns.length, session.pendingToolCalls.length, running]);

  const handleSubmit = async (prompt: string) => {
    if (onBeforeSend) {
      try {
        await onBeforeSend(prompt);
      } catch (err) {
        console.error("[ChatView] onBeforeSend rejected", err);
        return;
      }
    }
    session.send(prompt);
  };

  const handleReset = () => {
    // Run onReset *first* so callers can observe the old sessionId (to
    // unbind server-side state, cancel requests, etc.) before the hook
    // rotates it.
    onReset?.();
    session.reset();
  };

  return (
    <div className="mx-auto flex h-full w-full max-w-3xl flex-col">
      <header className="flex items-center justify-between border-b px-4 py-2">
        <div className="font-mono text-xs text-muted-foreground">
          session {session.sessionId.slice(0, 8)} · {session.status}
        </div>
        <div className="flex items-center gap-1">
          {headerExtra}
          {turns.length > 0 && (
            <Button variant="ghost" size="sm" onClick={handleReset}>
              New chat
            </Button>
          )}
          <ThemeToggle />
        </div>
      </header>

      <div className="flex-1 overflow-y-auto px-4 py-4 space-y-3">
        {turns.length === 0 && (
          renderEmptyState ? (
            renderEmptyState({ onPick: handleSubmit })
          ) : (
            <DefaultEmptyState onPick={handleSubmit} />
          )
        )}
        {turns.map((t) => (
          <TurnBubble
            key={t.key}
            turn={t}
            pendingByToolUseId={pendingByToolUseId}
            resolvedToolUseIds={resolvedToolUseIds}
            toolNameById={toolNameById}
            onResolve={session.resolveToolCall}
          />
        ))}
        {running && turns[turns.length - 1]?.role === "user" && <ThinkingBubble />}
        <div ref={endRef} />
      </div>

      {session.error && (
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap border-t bg-destructive/10 p-2 font-mono text-xs text-destructive">
          {session.error}
        </pre>
      )}

      {aboveComposer}

      <PromptInput
        onSubmit={handleSubmit}
        onCancel={session.cancel}
        disabled={running}
        running={running}
        leftAdornment={composerLeftAdornment}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Message → turn projection.
// ---------------------------------------------------------------------------

type Turn =
  | { role: "user"; key: number; text: string }
  | { role: "assistant"; key: number; blocks: AssistantBlock[] };

type AssistantBlock =
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool_use"; id: string; name: string; input: unknown }
  | { kind: "tool_result"; tool_use_id: string; content: unknown };

function projectTurns(messages: ClaudeMessage[]): Turn[] {
  const out: Turn[] = [];
  for (const m of messages) {
    const type = m["type"] as string | undefined;
    const inner = (m["message"] as any)?.content;
    if (type === "user") {
      if (typeof inner === "string") {
        out.push({ role: "user", key: m._seq, text: inner });
      } else if (Array.isArray(inner)) {
        const last = out[out.length - 1];
        if (last && last.role === "assistant") {
          for (const b of inner) {
            if (b?.type === "tool_result") {
              last.blocks.push({
                kind: "tool_result",
                tool_use_id: b.tool_use_id,
                content: b.content,
              });
            }
          }
        }
      }
    } else if (type === "assistant" && Array.isArray(inner)) {
      const blocks: AssistantBlock[] = [];
      for (const b of inner) {
        if (b?.type === "text") blocks.push({ kind: "text", text: b.text });
        else if (b?.type === "thinking")
          blocks.push({ kind: "thinking", text: b.thinking ?? "" });
        else if (b?.type === "tool_use") {
          blocks.push({ kind: "tool_use", id: b.id, name: b.name, input: b.input });
        }
      }
      const last = out[out.length - 1];
      if (last && last.role === "assistant") {
        last.blocks.push(...blocks);
      } else {
        out.push({ role: "assistant", key: m._seq, blocks });
      }
    }
  }
  return out;
}

function TurnBubble({
  turn,
  pendingByToolUseId,
  resolvedToolUseIds,
  toolNameById,
  onResolve,
}: {
  turn: Turn;
  pendingByToolUseId: Map<string, PendingToolCall>;
  resolvedToolUseIds: Set<string>;
  toolNameById: Map<string, string>;
  onResolve: (id: string, content: unknown) => void;
}) {
  if (turn.role === "user") {
    return (
      <div className="flex justify-end">
        <div className="max-w-[80%] rounded-2xl bg-primary px-4 py-2 text-sm text-primary-foreground">
          {turn.text}
        </div>
      </div>
    );
  }
  return (
    <div className="flex justify-start">
      <div className="w-full max-w-[85%] space-y-2 rounded-2xl bg-muted px-4 py-3 text-sm">
        {turn.blocks.map((b, i) => (
          <AssistantBlockView
            key={i}
            block={b}
            pendingByToolUseId={pendingByToolUseId}
            resolvedToolUseIds={resolvedToolUseIds}
            toolNameById={toolNameById}
            onResolve={onResolve}
          />
        ))}
      </div>
    </div>
  );
}

function AssistantBlockView({
  block,
  pendingByToolUseId,
  resolvedToolUseIds,
  toolNameById,
  onResolve,
}: {
  block: AssistantBlock;
  pendingByToolUseId: Map<string, PendingToolCall>;
  resolvedToolUseIds: Set<string>;
  toolNameById: Map<string, string>;
  onResolve: (id: string, content: unknown) => void;
}) {
  if (block.kind === "text") {
    return <Markdown>{block.text}</Markdown>;
  }
  if (block.kind === "thinking") {
    return (
      <details className="text-xs text-muted-foreground">
        <summary className="cursor-pointer">thinking…</summary>
        <Markdown className="pt-1 prose-xs">{block.text}</Markdown>
      </details>
    );
  }
  if (block.kind === "tool_use") {
    const name = stripMcpPrefix(block.name);
    const pending = pendingByToolUseId.get(block.id);
    const isResolved = resolvedToolUseIds.has(block.id);
    const Component = pending ? clientToolRegistry[pending.name] : undefined;

    if (!isResolved && pending && Component) {
      return (
        <div>
          <div className="mb-1 text-xs text-muted-foreground">
            Claude is asking: <code className="font-mono">{pending.name}</code>
          </div>
          <Component
            input={pending.input}
            resolve={(content) => onResolve(pending.tool_call_id, content)}
          />
        </div>
      );
    }
    return (
      <div className="rounded bg-background/50 p-2 text-xs">
        <span className="font-mono">tool</span> · {name}
      </div>
    );
  }
  if (block.kind === "tool_result") {
    const toolName = toolNameById.get(block.tool_use_id);
    const ResultComponent = toolName ? toolResultRegistry[toolName] : undefined;
    const parsed = parseToolResultContent(block.content);

    if (ResultComponent) {
      return (
        <ResultComponent
          content={parsed.value}
          toolUseId={block.tool_use_id}
        />
      );
    }
    return (
      <details className="text-xs text-muted-foreground">
        <summary className="cursor-pointer">tool result</summary>
        <pre className="mt-1 overflow-x-auto whitespace-pre-wrap rounded bg-background/50 p-2">
          {parsed.preview}
        </pre>
      </details>
    );
  }
  return null;
}

/** Strip the MCP server prefix (`mcp__<server>__`) off a tool name. */
function stripMcpPrefix(name: string): string {
  return name.replace(/^mcp__[^_]+(?:__)?/, "");
}

function stableStringify(value: unknown): string {
  return JSON.stringify(value, sortedReplacer);
}

function sortedReplacer(_key: string, value: unknown): unknown {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const sorted: Record<string, unknown> = {};
    for (const k of Object.keys(value as Record<string, unknown>).sort()) {
      sorted[k] = (value as Record<string, unknown>)[k];
    }
    return sorted;
  }
  return value;
}

function parseToolResultContent(raw: unknown): {
  value: unknown;
  preview: string;
} {
  const preview =
    typeof raw === "string"
      ? raw
      : Array.isArray(raw)
        ? raw
            .map((c: any) => (c?.type === "text" ? c.text : JSON.stringify(c)))
            .join("\n")
        : JSON.stringify(raw);
  try {
    return { value: JSON.parse(preview), preview };
  } catch {
    return { value: preview, preview };
  }
}

function DefaultEmptyState({ onPick }: { onPick: (prompt: string) => void }) {
  const suggestions = [
    "What questions can I ask about this dataset?",
    "Summarize the dataset: what columns and what trends stand out?",
    "Show me the distribution of the most important numeric column.",
  ];
  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-6 py-8">
      <div className="space-y-1 text-center">
        <h1 className="text-2xl font-semibold tracking-tight">
          Agentic Visualization
        </h1>
        <p className="text-sm text-muted-foreground">
          Upload a CSV or JSON dataset, then ask a question. Claude will
          interleave charts and explanations in the same bubble.
        </p>
      </div>
      <div className="rounded-lg border bg-card p-4 text-sm">
        <div className="mb-2 font-medium">How it works</div>
        <ol className="list-decimal space-y-1 pl-5 text-muted-foreground">
          <li>Click <b>Dataset</b> in the composer to upload a file.</li>
          <li>
            Ask a data question — e.g.{" "}
            <span className="italic">"what's the trend of car prices?"</span>
          </li>
          <li>
            Claude calls{" "}
            <code className="font-mono text-xs">describe_dataset</code>,{" "}
            <code className="font-mono text-xs">query_dataset</code>, and{" "}
            <code className="font-mono text-xs">create_chart</code> (Python
            MCP server) and streams prose + charts back.
          </li>
        </ol>
      </div>
      <div className="flex flex-col gap-2">
        <div className="text-center text-xs uppercase tracking-wide text-muted-foreground">
          Try a prompt
        </div>
        {suggestions.map((s) => (
          <Button
            key={s}
            variant="outline"
            size="sm"
            className="justify-start"
            onClick={() => onPick(s)}
          >
            {s}
          </Button>
        ))}
      </div>
    </div>
  );
}
