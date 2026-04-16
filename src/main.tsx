import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import {
  registerClientTool,
  registerToolResult,
} from "@/core/tools/registry";
import { ShowChoice } from "@/core/tools/builtins/ShowChoice";
import { FlightResults } from "@/core/tools/builtins/FlightResults";
import { WeatherResultCard } from "@/core/tools/builtins/WeatherResultCard";
import { ChoiceResultCard } from "@/core/tools/builtins/ChoiceResultCard";
import { FlightPickResultCard } from "@/core/tools/builtins/FlightPickResultCard";
import "./styles.css";

// Client-tool renderers — React components Claude can call. Every name
// here must match a tool declared on the backend with `b.client_tool(...)`.
// Forks add their own (SeatMap, PaymentForm, ...) here.
registerClientTool("show_choice", ShowChoice);
registerClientTool("show_flight_options", FlightResults);

// Tool-result renderers — custom bubbles for the `tool_result` block
// that follows a `tool_use`. Optional: unregistered tools fall back to
// a collapsed JSON preview. Register one per tool whose result deserves
// a richer presentation than raw JSON.
registerToolResult("get_weather", WeatherResultCard);
registerToolResult("show_choice", ChoiceResultCard);
registerToolResult("show_flight_options", FlightPickResultCard);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
