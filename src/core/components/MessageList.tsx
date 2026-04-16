import { useEffect, useRef } from "react";
import type { ClaudeMessage } from "@/core/hooks/useClaudeSession";

/**
 * Very literal renderer: dumps each stream-json line as a card. Forks
 * typically replace this with rich rendering (markdown, diff blocks, tool
 * use collapses, thinking chevrons, etc.) — this default exists so the
 * template works end-to-end the moment you clone it.
 */
export function MessageList({ messages }: { messages: ClaudeMessage[] }) {
  const endRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [messages.length]);

  return (
    <div className="flex-1 overflow-y-auto p-4 space-y-3">
      {messages.length === 0 && (
        <p className="text-sm text-muted-foreground italic">No messages yet.</p>
      )}
      {messages.map((m) => (
        <MessageCard key={m._seq} msg={m} />
      ))}
      <div ref={endRef} />
    </div>
  );
}

function MessageCard({ msg }: { msg: ClaudeMessage }) {
  const type = String(msg["type"] ?? "unknown");
  const label = labelForType(type, msg);

  // Extract a human-readable preview when possible; otherwise fall back to
  // the raw JSON so forks can see everything until they add custom rendering.
  const preview = renderPreview(msg);

  return (
    <div className="rounded border bg-card p-3 text-sm">
      <div className="mb-1 font-mono text-xs uppercase text-muted-foreground">{label}</div>
      {preview}
    </div>
  );
}

function labelForType(type: string, msg: ClaudeMessage): string {
  if (type === "assistant") return "assistant";
  if (type === "user") return "user";
  if (type === "result") return `result · $${(msg["total_cost_usd"] as number | undefined)?.toFixed(4) ?? "?"}`;
  if (type === "system") return `system · ${msg["subtype"] ?? ""}`;
  return type;
}

function renderPreview(msg: ClaudeMessage) {
  const inner = (msg["message"] as { content?: unknown } | undefined)?.content;
  if (Array.isArray(inner)) {
    return (
      <div className="space-y-2">
        {inner.map((block: any, i: number) => (
          <ContentBlock key={i} block={block} />
        ))}
      </div>
    );
  }
  if (typeof inner === "string") return <p className="whitespace-pre-wrap">{inner}</p>;
  if (msg["type"] === "result" && typeof msg["result"] === "string") {
    return <p className="whitespace-pre-wrap">{msg["result"] as string}</p>;
  }
  return (
    <pre className="overflow-x-auto rounded bg-muted p-2 text-xs">
      {JSON.stringify(msg, null, 2)}
    </pre>
  );
}

function ContentBlock({ block }: { block: any }) {
  if (block?.type === "text") return <p className="whitespace-pre-wrap">{block.text}</p>;
  if (block?.type === "thinking") {
    return (
      <details className="text-muted-foreground">
        <summary className="cursor-pointer text-xs">thinking…</summary>
        <p className="whitespace-pre-wrap pt-1">{block.thinking}</p>
      </details>
    );
  }
  if (block?.type === "tool_use") {
    return (
      <div className="rounded bg-muted p-2 text-xs">
        <span className="font-mono">tool_use</span> · {block.name}
        <pre className="mt-1 overflow-x-auto">{JSON.stringify(block.input, null, 2)}</pre>
      </div>
    );
  }
  if (block?.type === "tool_result") {
    return (
      <div className="rounded bg-muted p-2 text-xs">
        <span className="font-mono">tool_result</span>
        <pre className="mt-1 overflow-x-auto whitespace-pre-wrap">
          {typeof block.content === "string" ? block.content : JSON.stringify(block.content, null, 2)}
        </pre>
      </div>
    );
  }
  return (
    <pre className="overflow-x-auto rounded bg-muted p-2 text-xs">
      {JSON.stringify(block, null, 2)}
    </pre>
  );
}
