import { Cloud, CloudRain, Sun, Wind } from "lucide-react";
import type { ToolResultProps } from "@/core/tools/registry";

/**
 * Result renderer for the `get_weather` server tool. Paired with the
 * handler in `backend/src/main.rs`, which returns an object like
 * `{ location, temperature_c, summary }`. Keeps the JSON contract loose
 * (anything missing degrades gracefully) so forks can evolve the
 * handler without breaking the component.
 */
interface WeatherResult {
  location?: string;
  temperature_c?: number;
  summary?: string;
  _note?: string;
}

export function WeatherResultCard({
  content,
}: ToolResultProps<WeatherResult>) {
  const data = content ?? {};
  const temp = data.temperature_c;
  const summary = data.summary ?? "";
  const Icon = pickIcon(summary);

  return (
    <div className="flex items-center gap-3 rounded-lg border bg-card p-3">
      <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-muted">
        <Icon className="h-5 w-5" />
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="text-xs uppercase tracking-wide text-muted-foreground">
          Weather · {data.location ?? "unknown"}
        </div>
        <div className="flex items-baseline gap-2">
          <div className="text-xl font-semibold">
            {typeof temp === "number" ? `${Math.round(temp)}°C` : "—"}
          </div>
          <div className="truncate text-sm text-muted-foreground">
            {summary || "no conditions reported"}
          </div>
        </div>
      </div>
    </div>
  );
}

function pickIcon(summary: string) {
  const s = summary.toLowerCase();
  if (s.includes("rain") || s.includes("drizzle") || s.includes("shower"))
    return CloudRain;
  if (s.includes("wind") || s.includes("breezy") || s.includes("gust"))
    return Wind;
  if (s.includes("cloud") || s.includes("overcast")) return Cloud;
  return Sun;
}
