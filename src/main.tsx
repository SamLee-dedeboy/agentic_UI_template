import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { registerToolResult } from "@/core/tools/registry";
import { ChartResultCard } from "@/features/viz/ChartResultCard";
import "./styles.css";

// Tool-result renderers — custom bubbles for the `tool_result` block that
// follows a `tool_use`. `create_chart` returns a Vega-Lite spec that we
// render inline with react-vega. `describe_dataset` and `query_dataset`
// have no custom renderer — they fall through to the default JSON
// preview, which is fine: Claude reads them from its own context, not
// the user.
registerToolResult("create_chart", ChartResultCard);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
