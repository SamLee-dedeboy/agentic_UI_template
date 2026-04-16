import { Button } from "@/components/ui/button";
import type { ClientToolProps } from "@/core/tools/registry";

/**
 * Reference client-tool: renders a list of flights for the user to
 * choose from. Paired with the `search_flights` server tool in
 * `backend/src/main.rs`, the flow is:
 *
 *   1. User asks Claude to find flights.
 *   2. Claude calls `search_flights(origin, destination, date)` — server
 *      tool returns stubbed Flight[] JSON.
 *   3. Claude calls `show_flight_options({ origin, destination, date,
 *      flights })` — this component renders inline in the chat bubble.
 *   4. User clicks "Select" on a flight. `resolve` sends the picked id
 *      back up the WebSocket as a `tool_result`, unblocking Claude.
 *   5. Claude writes a confirmation ("Great, I've reserved AC123…").
 *
 * Forks typically copy this file, tweak the Flight shape + layout, and
 * re-register under a domain-specific tool name. The data contract is
 * what the server tool produces; this component is pure presentation +
 * one `resolve` call.
 */

export interface Flight {
  id: string;
  airline: string;
  flight_number: string;
  origin: string;
  destination: string;
  depart_time: string;
  arrive_time: string;
  duration_minutes: number;
  stops: number;
  price_usd: number;
  cabin?: string;
}

interface FlightResultsInput {
  origin: string;
  destination: string;
  date: string;
  flights: Flight[];
}

type FlightResultsResult = {
  picked_id: string;
  picked_flight: Flight;
};

export function FlightResults({
  input,
  resolve,
}: ClientToolProps<FlightResultsInput, FlightResultsResult>) {
  const { origin, destination, date, flights } = input;

  if (!flights || flights.length === 0) {
    return (
      <div className="rounded-lg border bg-card p-3 text-sm text-muted-foreground">
        No flights found for {origin} → {destination} on {date}.
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2 rounded-lg border bg-card p-3">
      <div className="flex items-baseline justify-between">
        <div className="text-sm font-medium">
          {origin} → {destination}
        </div>
        <div className="text-xs text-muted-foreground">{date}</div>
      </div>

      <ul className="flex flex-col gap-2">
        {flights.map((f) => (
          <li
            key={f.id}
            className="flex items-center gap-3 rounded-md border bg-background/60 p-3"
          >
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <div className="flex items-center gap-2 text-sm font-medium">
                <span>{f.airline}</span>
                <span className="font-mono text-xs text-muted-foreground">
                  {f.flight_number}
                </span>
                {f.cabin && (
                  <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted-foreground">
                    {f.cabin}
                  </span>
                )}
              </div>
              <div className="flex items-center gap-2 text-sm">
                <span className="font-mono">{f.depart_time}</span>
                <span className="text-muted-foreground">→</span>
                <span className="font-mono">{f.arrive_time}</span>
                <span className="text-xs text-muted-foreground">
                  · {formatDuration(f.duration_minutes)} ·{" "}
                  {f.stops === 0
                    ? "nonstop"
                    : `${f.stops} stop${f.stops > 1 ? "s" : ""}`}
                </span>
              </div>
            </div>

            <div className="flex flex-col items-end gap-1">
              <div className="text-sm font-semibold">
                ${f.price_usd.toLocaleString()}
              </div>
              <Button
                size="sm"
                onClick={() => resolve({ picked_id: f.id, picked_flight: f })}
              >
                Select
              </Button>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}

function formatDuration(minutes: number): string {
  if (!Number.isFinite(minutes) || minutes <= 0) return "—";
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h === 0) return `${m}m`;
  if (m === 0) return `${h}h`;
  return `${h}h ${m}m`;
}
