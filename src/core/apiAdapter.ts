/**
 * Web-only API adapter for the template.
 *
 * Two surfaces:
 *  - {@link apiCall}: REST for one-shot commands, WebSocket for streaming ones.
 *    The command-name input mirrors the old Tauri-style string so that forks
 *    carrying opcode-derived code don't have to rename call sites.
 *  - Session-scoped `CustomEvent`s on `window`: the streaming path dispatches
 *    `claude-output:<sessionId>`, `claude-error:<sessionId>`,
 *    `claude-complete:<sessionId>`, and `claude-cancelled:<sessionId>` so
 *    concurrent sessions don't cross-contaminate. Generic versions (without
 *    the `:<id>` suffix) are also dispatched for simple single-session UIs.
 *
 * Keep this file small — feature modules should build their own typed
 * wrappers in `src/features/<name>/api.ts` on top of `apiCall`.
 */

export interface ApiResponse<T> {
  success: boolean;
  data?: T;
  error?: string;
}

/** One-shot REST call against the Axum backend. */
async function restApiCall<T>(endpoint: string, params?: Record<string, unknown>): Promise<T> {
  let processed = endpoint;
  const pathParamKeys = new Set<string>();
  if (params) {
    for (const key of Object.keys(params)) {
      const variants = [
        `{${key}}`,
        `{${key.charAt(0).toLowerCase() + key.slice(1)}}`,
        `{${key.charAt(0).toUpperCase() + key.slice(1)}}`,
      ];
      for (const v of variants) {
        if (processed.includes(v)) {
          processed = processed.replace(v, encodeURIComponent(String(params[key])));
          pathParamKeys.add(key);
        }
      }
    }
  }

  const url = new URL(processed, window.location.origin);
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (pathParamKeys.has(k)) continue;
      if (v == null) continue;
      url.searchParams.append(k, String(v));
    }
  }

  const res = await fetch(url.toString(), {
    method: "GET",
    headers: authHeaders(),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status} ${res.statusText}`);
  const body = (await res.json()) as ApiResponse<T>;
  if (!body.success) throw new Error(body.error ?? "API call failed");
  return body.data as T;
}

/** Optional `Authorization: Bearer <token>` header from a global set via Phase 3. */
function authHeaders(): Record<string, string> {
  const token = (globalThis as any).__APP_AUTH_TOKEN;
  return token ? { "Content-Type": "application/json", Authorization: `Bearer ${token}` } : { "Content-Type": "application/json" };
}

const STREAMING_COMMANDS = new Set([
  "execute_claude_code",
  "continue_claude_code",
  "resume_claude_code",
]);

// Map of live session ID → its WebSocket. We keep one entry per session
// so `resolveClientToolCall` can find the right socket to reply on when a
// client-tool result is ready. Entries are added on open and removed on
// close. This lives at module scope because tool resolution happens
// outside the `apiCall` Promise that opened the socket.
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
 * Unified dispatch. Pass the Tauri-style command name and the request params;
 * streaming commands open a WebSocket, non-streaming ones hit REST.
 */
export async function apiCall<T>(command: string, params?: Record<string, unknown>): Promise<T> {
  if (STREAMING_COMMANDS.has(command)) {
    return handleStreamingCommand<T>(command, params);
  }
  return restApiCall<T>(mapCommandToEndpoint(command), params);
}

/**
 * Tauri-style command → REST endpoint. Only the subset the backend actually
 * serves today is listed; anything else falls through with a warning so forks
 * notice missing endpoints early.
 */
function mapCommandToEndpoint(command: string): string {
  const table: Record<string, string> = {
    list_projects: "/api/projects",
    get_project_sessions: "/api/projects/{projectId}/sessions",
    get_claude_settings: "/api/settings/claude",
    check_claude_version: "/api/settings/claude/version",
    list_claude_installations: "/api/settings/claude/installations",
    get_system_prompt: "/api/settings/system-prompt",
    open_new_session: "/api/sessions/new",
    load_session_history: "/api/sessions/{sessionId}/history/{projectId}",
    list_running_claude_sessions: "/api/sessions/running",
    list_slash_commands: "/api/slash-commands",
    mcp_list: "/api/mcp/servers",
    cancel_claude_execution: "/api/sessions/{sessionId}/cancel",
    get_claude_session_output: "/api/sessions/{sessionId}/output",
  };
  const endpoint = table[command];
  if (!endpoint) {
    console.warn(
      `[apiAdapter] unknown command '${command}'; the backend has no REST endpoint for it. ` +
        `Add one in backend/src/web_server.rs or route through a feature-specific api module.`,
    );
    return `/api/unknown/${command}`;
  }
  return endpoint;
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
