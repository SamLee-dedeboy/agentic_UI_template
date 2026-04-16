/**
 * Web-only API adapter for the template.
 *
 * Two surfaces:
 *  - {@link apiCall}: opens a WebSocket for one of the streaming Claude
 *    commands (`execute_claude_code` / `continue_claude_code` /
 *    `resume_claude_code`) and resolves when the turn completes.
 *  - Session-scoped `CustomEvent`s on `window`: the streaming path
 *    dispatches `claude-output:<sessionId>`, `claude-error:<sessionId>`,
 *    `claude-complete:<sessionId>`, `claude-cancelled:<sessionId>`, and
 *    `claude-tool-call:<sessionId>` so concurrent sessions don't
 *    cross-contaminate. Generic versions (without the `:<id>` suffix)
 *    are also dispatched for simple single-session UIs.
 *
 * Keep this file small — feature modules should build their own typed
 * wrappers on top of `apiCall` and the named helpers below.
 */

export interface ApiResponse<T> {
  success: boolean;
  data?: T;
  error?: string;
}

const STREAMING_COMMANDS = new Set([
  "execute_claude_code",
  "continue_claude_code",
  "resume_claude_code",
]);

// Map of live session ID → its WebSocket. We keep one entry per session
// so `resolveClientToolCall` can find the right socket to reply on when a
// client-tool result is ready. Entries are added on open and removed on
// close. Module scope because tool resolution happens outside the
// `apiCall` Promise that opened the socket.
const liveSessionSockets = new Map<string, WebSocket>();

/**
 * Reply to a pending client-tool call. The backend dispatch endpoint is
 * `await`ing this `content` on a oneshot channel keyed by `toolCallId`.
 * `content` can be any JSON-serializable value — it becomes the body of
 * the `tool_result` block Claude sees.
 */
export function resolveClientToolCall(
  sessionId: string,
  toolCallId: string,
  content: unknown,
): void {
  const ws = liveSessionSockets.get(sessionId);
  if (!ws || ws.readyState !== WebSocket.OPEN) {
    console.warn(
      `[apiAdapter] no open socket for session ${sessionId}; tool call ${toolCallId} dropped`,
    );
    return;
  }
  ws.send(
    JSON.stringify({
      type: "tool_result_from_ui",
      tool_call_id: toolCallId,
      content,
    }),
  );
}

/**
 * Close a session's WebSocket and drop it from the live map. Used by
 * `useClaudeSession.reset` to ensure a retired session can't leak stream
 * events into the newly-started session.
 */
export function closeSession(sessionId: string): void {
  const ws = liveSessionSockets.get(sessionId);
  if (ws) {
    try {
      ws.close(1000, "session reset");
    } catch {
      // Already closing/closed — nothing to do.
    }
    liveSessionSockets.delete(sessionId);
  }
}

/**
 * Open a streaming Claude session over WebSocket. Resolves when the run
 * completes (success or failure); use the session-scoped `CustomEvent`s
 * to consume the intermediate stream-json.
 */
export async function apiCall<T>(
  command: string,
  params?: Record<string, unknown>,
): Promise<T> {
  if (!STREAMING_COMMANDS.has(command)) {
    throw new Error(
      `[apiAdapter] unknown command '${command}'. The template only wires streaming commands (${[...STREAMING_COMMANDS].join(", ")}). Forks that need non-streaming endpoints should add their own helpers in src/features/<name>/api.ts.`,
    );
  }
  return handleStreamingCommand<T>(command, params);
}

/** Internal: stream Claude output over WebSocket and synthesize session-scoped events. */
async function handleStreamingCommand<T>(
  command: string,
  params?: Record<string, any>,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const wsProtocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const token = (globalThis as any).__APP_AUTH_TOKEN;
    const query = token ? `?token=${encodeURIComponent(token)}` : "";
    const wsUrl = `${wsProtocol}//${window.location.host}/ws/claude${query}`;

    const clientSessionId: string =
      params?.clientSessionId ||
      (typeof crypto !== "undefined" && crypto.randomUUID
        ? crypto.randomUUID()
        : `client-${Date.now()}-${Math.random().toString(36).slice(2)}`);

    const ws = new WebSocket(wsUrl);
    liveSessionSockets.set(clientSessionId, ws);

    ws.onopen = () => {
      const request = {
        command_type: command.replace("_claude_code", ""),
        project_path: params?.projectPath ?? "",
        prompt: params?.prompt ?? "",
        model: params?.model ?? "sonnet",
        session_id: params?.sessionId,
        client_session_id: clientSessionId,
        extra: params?.extra ?? {},
      };
      ws.send(JSON.stringify(request));
    };

    const dispatch = (name: string, detail: unknown, sid?: string) => {
      window.dispatchEvent(new CustomEvent(name, { detail }));
      if (sid) window.dispatchEvent(new CustomEvent(`${name}:${sid}`, { detail }));
    };

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === "output") {
          try {
            const claudeMessage =
              typeof msg.content === "string" ? JSON.parse(msg.content) : msg.content;
            const sid: string | undefined = claudeMessage?.session_id || clientSessionId;
            dispatch("claude-output", claudeMessage, sid);
          } catch (e) {
            console.error("[apiAdapter] bad claude output content", e, msg.content);
          }
        } else if (msg.type === "completion") {
          dispatch("claude-complete", msg.status === "success", clientSessionId);
          ws.close();
          if (msg.status === "success") resolve({} as T);
          else reject(new Error(msg.error || "Execution failed"));
        } else if (msg.type === "error") {
          dispatch("claude-error", msg.message || "Unknown error", clientSessionId);
        } else if (msg.type === "cancelled") {
          dispatch("claude-cancelled", true, clientSessionId);
        } else if (msg.type === "tool_call_for_ui") {
          // The backend is blocked awaiting a `tool_result_from_ui` for
          // this `tool_call_id`. We hand the call off to whatever tool
          // registry the UI has wired up via a session-scoped event; the
          // consumer invokes `resolveClientToolCall` when the user acts.
          dispatch(
            "claude-tool-call",
            {
              tool_call_id: msg.tool_call_id,
              name: msg.name,
              input: msg.input,
            },
            clientSessionId,
          );
        }
      } catch (e) {
        console.error("[apiAdapter] bad WebSocket message", e, event.data);
      }
    };

    ws.onerror = (err) => {
      console.error("[apiAdapter] WebSocket error", err);
      dispatch("claude-error", "WebSocket connection failed", clientSessionId);
      reject(new Error("WebSocket connection failed"));
    };

    ws.onclose = (event) => {
      liveSessionSockets.delete(clientSessionId);
      // 1000/1001 are clean closes; anything else is an unexpected drop and
      // we surface that as a failed completion so UI state doesn't stick.
      if (event.code !== 1000 && event.code !== 1001) {
        dispatch("claude-complete", false, clientSessionId);
      }
    };
  });
}
