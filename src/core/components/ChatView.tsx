import { useEffect, useMemo, useRef } from "react";
import {
  useClaudeSession,
  type ClaudeMessage,
  type PendingToolCall,
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
 * This is the seam forks edit most. Swap bubble styling, add avatars,
 * add rich tool-result cards, etc. The logical core (message projection,
 * pending-call correlation, tool-call routing) stays here and in
 * `useClaudeSession`.
 */
export function ChatView({ model }: { model?: string }) {
  const session = useClaudeSession({ model });
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

  // Set of `tool_use.id`s that already have a `tool_result`. Used to
  // stop rendering the live input component once Claude has received a
  // result for that call (either because the user resolved it or the
  // backend timed out).
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
  // matching tool name + JSON-equal input. FIFO on both sides, so two
  // pending calls of the same tool with the same input get paired in
  // insertion order (rare but defensive).
  //
  // The two IDs are unrelated on the wire — backend-generated
  // `tool_call_id` vs. Claude-generated `tool_use.id` — so we need this
  // matching pass to render the live component alongside the right
  // bubble. See docs/tools.md for the full id story.
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

  // Safety net: if a `tool_result` arrived for a pending call (e.g. the
  // backend timed out and Claude got an error result), the live
  // component would otherwise sit forever. Drop the pending entry.
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
    // `session.removePendingToolCall` is stable (useCallback).
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

  return (
    <div className="mx-auto flex h-full w-full max-w-3xl flex-col">
      <header className="flex items-center justify-between border-b px-4 py-2">
        <div className="font-mono text-xs text-muted-foreground">
          session {session.sessionId.slice(0, 8)} · {session.status}
        </div>
        <div className="flex items-center gap-1">
          {turns.length > 0 && (
            <Button variant="ghost" size="sm" onClick={session.reset}>
              New chat
            </Button>
          )}
          <ThemeToggle />
        </div>
      </header>

      <div className="flex-1 overflow-y-auto px-4 py-4 space-y-3">
        {turns.length === 0 && (
          <EmptyState suggestions={suggestionsFor(session)} onPick={session.send} />
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
        {/* Show the thinking animation while a prompt is in flight and
            no assistant turn has started yet (i.e., Claude hasn't
            produced a text/tool_use block for this turn). */}
        {running && turns[turns.length - 1]?.role === "user" && (
          <ThinkingBubble />
        )}
        <div ref={endRef} />
      </div>

      {session.error && (
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap border-t bg-destructive/10 p-2 font-mono text-xs text-destructive">
          {session.error}
        </pre>
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

// ---------------------------------------------------------------------------
// Message → turn projection.
// Stream-json arrives as a flat sequence of {type: "user"|"assistant"|...}.
// For display we group contiguous entries into "turns" and extract the
// rendering-relevant bits.
// ---------------------------------------------------------------------------

type Turn =
  | {
      role: "user";
      key: number;
      text: string;
    }
  | {
      role: "assistant";
      key: number;
      blocks: AssistantBlock[];
    };

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
    const ResultComponent = toolName
      ? toolResultRegistry[toolName]
      : undefined;
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

/** Strip the MCP server prefix (`mcp__template-tools__`) off a tool name. */
function stripMcpPrefix(name: string): string {
  return name.replace(/^mcp__[^_]+(?:__)?/, "");
}

/**
 * Order-stable JSON stringify used to compare tool inputs across
 * Claude's stream-json and the backend's `tool_call_for_ui` payload.
 * Object key order isn't guaranteed to match across the two paths, so
 * sort before stringifying.
 */
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

/**
 * The wire shape of `tool_result.content` is either a plain string or
 * an array of content blocks like `[{type:"text", text:"<json>"}]`
 * (the bridge currently always emits the array form). Flatten to a
 * single text preview, then try to parse as JSON so custom renderers
 * get a structured value.
 */
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

function EmptyState({
  suggestions,
  onPick,
}: {
  suggestions: string[];
  onPick: (prompt: string) => void;
}) {
  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-6 py-8">
      <div className="space-y-1 text-center">
        <h1 className="text-2xl font-semibold tracking-tight">
          Claude UI Template
        </h1>
        <p className="text-sm text-muted-foreground">
          A chat where Claude calls your tools and your React components render
          the results.
        </p>
      </div>

      <div className="grid gap-3 sm:grid-cols-3">
        <ExplainCard
          title="Server tools"
          body="Rust handlers Claude invokes over MCP. Declare the schema + handler in one place."
          file="backend/src/main.rs"
          example="b.server_tool(&quot;get_weather&quot;, …)"
        />
        <ExplainCard
          title="Client tools"
          body="React components that render inline in chat. resolve(value) returns a tool result."
          file="src/main.tsx"
          example="registerClientTool(&quot;show_choice&quot;, ShowChoice)"
        />
        <ExplainCard
          title="Chat shell"
          body="This view. Owns the WebSocket session, streams stream-json into bubbles."
          file="src/core/components/ChatView.tsx"
          example="const s = useClaudeSession()"
        />
      </div>

      <div className="rounded-lg border bg-card p-4 text-sm">
        <div className="mb-2 font-medium">How a turn flows</div>
        <ol className="list-decimal space-y-1 pl-5 text-muted-foreground">
          <li>You send a prompt from the input below.</li>
          <li>
            Backend spawns <code className="font-mono text-xs">claude</code>{" "}
            with <code className="font-mono text-xs">--mcp-config</code>{" "}
            pointing at <code className="font-mono text-xs">tool-bridge</code>.
          </li>
          <li>
            Claude decides to call a tool. Server tools run in Rust; client
            tools round-trip to this browser.
          </li>
          <li>
            Stream-json flows back over WebSocket and renders as bubbles
            (text, thinking, tool_use, tool_result).
          </li>
        </ol>
      </div>

      <div className="flex flex-col gap-2">
        <div className="text-center text-xs uppercase tracking-wide text-muted-foreground">
          Try a prompt
        </div>
        <div className="flex flex-col gap-2">
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
        <p className="text-center text-xs text-muted-foreground">
          Suggestions exercise the three example tools:{" "}
          <code className="font-mono">get_weather</code> (server),{" "}
          <code className="font-mono">show_choice</code> (client), and the{" "}
          <code className="font-mono">search_flights</code> +{" "}
          <code className="font-mono">show_flight_options</code> pair
          (server → client).
        </p>
      </div>

      <div className="border-t pt-4 text-center text-xs text-muted-foreground">
        Edit <code className="font-mono">src/App.tsx</code> and save — Vite HMR
        reloads instantly. See{" "}
        <code className="font-mono">docs/tools.md</code> for the full tool
        guide, <code className="font-mono">CLAUDE.md</code> for architecture.
      </div>
    </div>
  );
}

function ExplainCard({
  title,
  body,
  file,
  example,
}: {
  title: string;
  body: string;
  file: string;
  example: string;
}) {
  return (
    <div className="flex flex-col gap-2 rounded-lg border bg-card p-3 text-left">
      <div className="text-sm font-medium">{title}</div>
      <p className="text-xs text-muted-foreground">{body}</p>
      <div className="mt-auto space-y-1">
        <div className="truncate font-mono text-[11px] text-muted-foreground">
          {file}
        </div>
        <pre className="overflow-x-auto rounded bg-muted/60 p-1.5 font-mono text-[11px] leading-snug">
          {example}
        </pre>
      </div>
    </div>
  );
}

// One suggestion per shipped reference tool / tool-pair. A fork that
// swaps the registry should swap these too so the empty state stays
// useful.
function suggestionsFor(_: ReturnType<typeof useClaudeSession>): string[] {
  return [
    "What's the weather like in Tokyo?",
    "Help me pick a color for a new website: blue, green, or purple.",
    "Find me flights from SFO to Tokyo on 2026-05-10.",
  ];
}
