import { useEffect, useMemo, useRef, useState } from "react";
import { VegaLite } from "react-vega";
import type { ToolResultProps } from "@/core/tools/registry";

// react-vega's exported `VisualizationSpec` has moved around across
// versions; locally we just need something loose enough to pass through
// to <VegaLite />. Full validation happens on the Python side (Altair
// emitted the spec, so it's valid Vega-Lite).
type VisualizationSpec = Record<string, unknown>;

/**
 * Result bubble for the `create_chart` tool. The Python MCP server
 * returns `{vega_lite_spec, summary, row_count}`; we render the spec
 * inline via react-vega.
 *
 * Sized to the bubble's container width and adapts to the app's
 * light/dark theme by overriding the Vega-Lite `background` and axis
 * text colors.
 */
interface ChartContent {
  vega_lite_spec?: VisualizationSpec;
  summary?: string;
  row_count?: number;
}

export function ChartResultCard({ content }: ToolResultProps<ChartContent>) {
  // react-vega's `width: "container"` path often initial-measures at 0
  // (the SVG renders with `width="0"` and never recovers even with
  // autosize=fit). Measure the wrapper ourselves and pass a concrete
  // numeric width into the spec so Vega never sees "container".
  const wrapRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    setWidth(Math.floor(el.getBoundingClientRect().width));
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const w = Math.floor(entry.contentRect.width);
        setWidth((prev) => (Math.abs(prev - w) > 1 ? w : prev));
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const spec = content?.vega_lite_spec;
  const themed = useMemo(() => {
    if (!spec || typeof spec !== "object") return null;
    if (width < 1) return null;
    const s = spec as any;
    const config = s.config || {};
    return {
      ...s,
      width,
      background: "transparent",
      autosize: { type: "fit", contains: "padding" },
      config: {
        ...config,
        axis: {
          ...(config.axis || {}),
          labelColor: "currentColor",
          titleColor: "currentColor",
          gridColor: "rgba(128,128,128,0.2)",
          domainColor: "rgba(128,128,128,0.4)",
          tickColor: "rgba(128,128,128,0.4)",
        },
        legend: {
          ...(config.legend || {}),
          labelColor: "currentColor",
          titleColor: "currentColor",
        },
        title: {
          ...(config.title || {}),
          color: "currentColor",
        },
        view: {
          ...(config.view || {}),
          stroke: "transparent",
        },
      },
    } as VisualizationSpec;
  }, [spec, width]);

  if (!spec) {
    return (
      <div className="rounded border border-destructive/40 bg-destructive/10 p-2 text-xs text-destructive">
        chart tool returned no spec
      </div>
    );
  }

  return (
    <div className="w-full overflow-hidden rounded border bg-card p-3">
      {content?.summary && (
        <div className="mb-2 text-xs uppercase tracking-wide text-muted-foreground">
          {content.summary}
          {typeof content.row_count === "number" && (
            <span className="ml-2 normal-case">
              ({content.row_count} points)
            </span>
          )}
        </div>
      )}
      <div ref={wrapRef} className="w-full">
        {themed && (
          <VegaLite spec={themed as any} actions={false} renderer="svg" />
        )}
      </div>
    </div>
  );
}
