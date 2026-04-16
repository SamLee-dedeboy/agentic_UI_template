import { PlaneTakeoff } from "lucide-react";
import type { ToolResultProps } from "@/core/tools/registry";
import type { Flight } from "@/core/tools/builtins/FlightResults";

/**
 * Result renderer for the `show_flight_options` client tool. Paired
 * with `FlightResults.tsx`, which resolves with
 * `{ picked_id, picked_flight }` when the user clicks "Select". This
 * card replaces the raw JSON preview with a readable confirmation so
 * the subsequent chat turn ("Great, I've reserved AC123…") has visible
 * context.
 */
interface FlightPickResult {
  picked_id?: string;
  picked_flight?: Flight;
}

export function FlightPickResultCard({
  content,
}: ToolResultProps<FlightPickResult>) {
  const f = content?.picked_flight;
  if (!f) {
    return (
      <div className="rounded-lg border bg-card p-3 text-sm text-muted-foreground">
        Selected flight {content?.picked_id ?? "(unknown)"}
      </div>
    );
  }
  return (
    <div className="flex items-center gap-3 rounded-lg border bg-card p-3">
      <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-muted">
        <PlaneTakeoff className="h-5 w-5" />
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="text-xs uppercase tracking-wide text-muted-foreground">
          Flight selected
        </div>
        <div className="flex items-baseline gap-2 truncate">
          <span className="font-medium">{f.airline}</span>
          <span className="font-mono text-xs text-muted-foreground">
            {f.flight_number}
          </span>
          <span className="truncate text-sm text-muted-foreground">
            {f.origin} → {f.destination} · {f.depart_time}
          </span>
        </div>
      </div>
      <div className="text-sm font-semibold">
        ${f.price_usd.toLocaleString()}
      </div>
    </div>
  );
}
