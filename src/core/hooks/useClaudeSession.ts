/**
 * Minimal Claude-session hook.
 *
 * Owns one conversation: generates a client session UUID, kicks off an
 * `execute_claude_code` call through the api adapter, listens for
 * session-scoped DOM events dispatched by `apiAdapter.ts`, and exposes the
 * accumulated messages + a cancel function.
 *
 * Forks that want richer session state (resume, tool-approval UI, branching)
 * should wrap this hook rather than fork it.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import {
  apiCall,
  closeSession,
  resolveClientToolCall,
} from "@/core/apiAdapter";

function freshSessionId(): string {
  if (typeof crypto !== "undefined" && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  return `client-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export type ClaudeMessage = {
  /** Local ordering key (monotonic). */
  _seq: number;
  /** Raw JSON from `claude`'s stream-json output, or synthesized error. */
  [key: string]: unknown;
};

export type ClaudeSessionStatus = "idle" | "running" | "completed" | "error" | "cancelled";

/**
 * An outstanding client-tool call awaiting a UI response. The backend is
 * blocked on a oneshot channel keyed by `tool_call_id`; consumers render
 * the corresponding component (looked up by `name` in the tool registry)
 * and invoke `resolveToolCall` with the user's result.
 */
export interface PendingToolCall {
  tool_call_id: string;
  name: string;
  input: unknown;
}

export interface UseClaudeSessionOptions {
  /**
   * Absolute path on the server host that Claude runs in. Optional — for
   * customer-facing apps Claude doesn't touch the filesystem so no cwd is
   * needed. Only supply this for dev/internal-tool forks.
   */
  projectPath?: string;
  /** Override the client-generated UUID. Rarely needed. */
  clientSessionId?: string;
  /** Model alias or full ID. Defaults to `sonnet`. */
  model?: string;
  /** Arbitrary extra flags forwarded to the backend's `ClaudeExtraArgs`. */
  extra?: Record<string, unknown>;
}

export type UseClaudeSessionReturn = ReturnType<typeof useClaudeSession>;

export function useClaudeSession(opts: UseClaudeSessionOptions = {}) {
  // Session ID is state, not memo: `reset()` regenerates it so a new
  // chat gets a fresh backend session (different sessionId => the WS
  // handler, conversation row, pending-tool maps, and cancel channel
  // all start clean). If the caller pins `clientSessionId` via opts we
  // honor it; otherwise we own generation.
  const [sessionId, setSessionId] = useState<string>(
    () => opts.clientSessionId ?? freshSessionId(),
  );
  // If the caller changes the pinned id, follow it.
  useEffect(() => {
    if (opts.clientSessionId) setSessionId(opts.clientSessionId);
  }, [opts.clientSessionId]);

  const [messages, setMessages] = useState<ClaudeMessage[]>([]);
  const [status, setStatus] = useState<ClaudeSessionStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [pendingToolCalls, setPendingToolCalls] = useState<PendingToolCall[]>([]);
  const seqRef = useRef(0);

  const append = useCallback((payload: unknown) => {
    seqRef.current += 1;
    setMessages((prev) => [
      ...prev,
      { _seq: seqRef.current, ...(payload as object) } as ClaudeMessage,
    ]);
  }, []);

  // Subscribe to session-scoped events dispatched by apiAdapter.
  useEffect(() => {
    const outHandler = (e: Event) => append((e as CustomEvent).detail);
    const errHandler = (e: Event) => {
      // Claude emits one error event per stderr line. Accumulate so the
      // user sees the full error context, not just the last line (which
      // is usually the generic exit-code message).
      const line = String((e as CustomEvent).detail);
      setError((prev) => (prev ? `${prev}\n${line}` : line));
      setStatus("error");
    };
    const completeHandler = (e: Event) => {
      const ok = Boolean((e as CustomEvent).detail);
      setStatus(ok ? "completed" : "error");
    };
    const cancelHandler = () => setStatus("cancelled");
    const toolCallHandler = (e: Event) => {
      const detail = (e as CustomEvent).detail as PendingToolCall;
      setPendingToolCalls((prev) => [...prev, detail]);
    };

    window.addEventListener(`claude-output:${sessionId}`, outHandler);
    window.addEventListener(`claude-error:${sessionId}`, errHandler);
    window.addEventListener(`claude-complete:${sessionId}`, completeHandler);
    window.addEventListener(`claude-cancelled:${sessionId}`, cancelHandler);
    window.addEventListener(`claude-tool-call:${sessionId}`, toolCallHandler);
    return () => {
      window.removeEventListener(`claude-output:${sessionId}`, outHandler);
      window.removeEventListener(`claude-error:${sessionId}`, errHandler);
      window.removeEventListener(`claude-complete:${sessionId}`, completeHandler);
      window.removeEventListener(`claude-cancelled:${sessionId}`, cancelHandler);
      window.removeEventListener(`claude-tool-call:${sessionId}`, toolCallHandler);
    };
  }, [sessionId, append]);

  /**
   * Resolve a pending client-tool call. Removes it from the local queue
   * and forwards the result to the backend over the session's WebSocket;
   * Claude picks it up as a `tool_result` on its next turn.
   */
  const resolveToolCall = useCallback(
    (toolCallId: string, content: unknown) => {
      resolveClientToolCall(sessionId, toolCallId, content);
      setPendingToolCalls((prev) =>
        prev.filter((p) => p.tool_call_id !== toolCallId),
      );
    },
    [sessionId],
  );

  /**
   * Drop a pending client-tool call without sending a result — e.g. when
   * the stream has already delivered a `tool_result` for it (backend
   * timed out, or the turn was cancelled), so the live component is
   * stale. `resolveToolCall` still calls this implicitly; this exposes
   * it for the cleanup path.
   */
  const removePendingToolCall = useCallback((toolCallId: string) => {
    setPendingToolCalls((prev) =>
      prev.filter((p) => p.tool_call_id !== toolCallId),
    );
  }, []);

  // How many turns have completed on this session. First call goes to
  // `execute_claude_code` (creates the Claude session); subsequent calls
  // go to `resume_claude_code` so the conversation has history.
  const turnCountRef = useRef(0);

  const send = useCallback(
    async (prompt: string) => {
      setStatus("running");
      setError(null);
      // Optimistic user turn — Claude Code's stream-json doesn't echo
      // the prompt back, so without this the user's message wouldn't
      // render in the chat until (and unless) Claude replies with a
      // tool_result that references it. Shape mirrors what the backend
      // would have emitted, so `projectTurns` picks it up unchanged.
      append({ type: "user", message: { role: "user", content: prompt } });
      const isFirstTurn = turnCountRef.current === 0;
      const command = isFirstTurn ? "execute_claude_code" : "resume_claude_code";
      turnCountRef.current += 1;
      try {
        await apiCall(command, {
          projectPath: opts.projectPath ?? "",
          prompt,
          model: opts.model ?? "sonnet",
          clientSessionId: sessionId,
          // For resume, the backend reads the conversation UUID from
          // `sessionId` (the prior conversation we want to continue).
          sessionId: isFirstTurn ? undefined : sessionId,
          extra: opts.extra,
        });
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setStatus("error");
      }
    },
    [append, opts.projectPath, opts.model, opts.extra, sessionId],
  );

  const cancel = useCallback(async () => {
    try {
      await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/cancel`, {
        method: "GET",
      });
    } catch (e) {
      console.warn("[useClaudeSession] cancel request failed", e);
    }
  }, [sessionId]);

  // Refs so `reset` can read the current sessionId/status without
  // becoming a churning callback (which would re-subscribe event
  // listeners on every tick).
  const sessionIdRef = useRef(sessionId);
  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);
  const statusRef = useRef(status);
  useEffect(() => {
    statusRef.current = status;
  }, [status]);

  const reset = useCallback(() => {
    const oldId = sessionIdRef.current;
    // Cancel a still-running backend session, then drop its socket so
    // late-arriving stream events can't bleed into the new chat.
    if (statusRef.current === "running") {
      fetch(`/api/sessions/${encodeURIComponent(oldId)}/cancel`, {
        method: "GET",
      }).catch((e) =>
        console.warn("[useClaudeSession] cancel on reset failed", e),
      );
    }
    closeSession(oldId);
    setMessages([]);
    setStatus("idle");
    setError(null);
    setPendingToolCalls([]);
    seqRef.current = 0;
    turnCountRef.current = 0;
    if (!opts.clientSessionId) setSessionId(freshSessionId());
  }, [opts.clientSessionId]);

  return {
    sessionId,
    messages,
    status,
    error,
    send,
    cancel,
    reset,
    pendingToolCalls,
    resolveToolCall,
    removePendingToolCall,
  };
}
