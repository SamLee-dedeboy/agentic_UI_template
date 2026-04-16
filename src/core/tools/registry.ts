import type { ComponentType } from "react";

/**
 * Props that every client-tool component receives.
 *
 * `input` is the arguments Claude passed in its `tool_use` block (already
 * typed on the fork side if desired). `resolve` is the back-channel: the
 * component calls it exactly once when the user has produced a result.
 * That value becomes the `tool_result` Claude sees on its next turn.
 */
export interface ClientToolProps<TInput = unknown, TResult = unknown> {
  input: TInput;
  resolve: (result: TResult) => void;
}

/**
 * Per-app map of tool name → React component. Forks mutate this in the
 * app-entry file (`main.tsx` or wherever they build the tree). The
 * default registry ships with `show_choice` as a working reference;
 * everything else is left for the fork to register.
 *
 * Consistency requirement: names in the frontend registry must match
 * tool names registered with the backend `ToolRegistry`. Backend
 * publishes them as `mcp__template-tools__<name>`, but inside the
 * frontend we only ever see the unprefixed form.
 */
export type ClientToolRegistry = Record<string, ComponentType<ClientToolProps>>;

// Default registry, built with primitive components shipped in
// `src/core/tools/builtins/`. Forks replace or extend this by importing
// `clientToolRegistry` and mutating it — or by constructing their own
// and passing it down via a context provider if they want isolation.
export const clientToolRegistry: ClientToolRegistry = {};

/** Convenience for a fork's `main.tsx`. */
export function registerClientTool<TInput = unknown, TResult = unknown>(
  name: string,
  component: ComponentType<ClientToolProps<TInput, TResult>>,
): void {
  clientToolRegistry[name] = component as ComponentType<ClientToolProps>;
}

/**
 * Props for a tool-result renderer. `content` is the parsed JSON value
 * Claude saw as the tool's result — for server tools this is whatever
 * your Rust handler returned; for client tools it's whatever the live
 * component `resolve()`d with. If parsing failed (plain-text result or
 * malformed JSON), `content` is the raw text string instead.
 */
export interface ToolResultProps<TContent = unknown> {
  content: TContent;
  toolUseId: string;
}

/**
 * Per-app map of tool name → React component for rendering the tool's
 * **result** bubble. Optional: tools without a registered renderer fall
 * back to a collapsed `<details>` JSON preview in `<ChatView>`.
 *
 * Server and client tools are treated identically here — what Claude
 * sees as the `tool_result` is the same wire shape either way.
 */
export type ToolResultRegistry = Record<string, ComponentType<ToolResultProps>>;

export const toolResultRegistry: ToolResultRegistry = {};

/** Convenience for a fork's `main.tsx`. */
export function registerToolResult<TContent = unknown>(
  name: string,
  component: ComponentType<ToolResultProps<TContent>>,
): void {
  toolResultRegistry[name] = component as ComponentType<ToolResultProps>;
}
