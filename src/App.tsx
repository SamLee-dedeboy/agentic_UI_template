import { useState } from "react";
import { ChatView } from "@/core/components/ChatView";
import { useClaudeSession } from "@/core/hooks/useClaudeSession";
import { DatasetChip } from "@/features/datasets/DatasetChip";
import { DatasetUploadButton } from "@/features/datasets/DatasetUploadButton";
import {
  bindDataset,
  unbindDataset,
} from "@/features/datasets/api";
import type { UploadedDataset } from "@/features/datasets/types";

/**
 * Agentic-viz shell.
 *
 * Owns:
 *   - the Claude session (so we can read `sessionId` when binding)
 *   - the currently-uploaded dataset (if any)
 *   - the bind/unbind lifecycle that tells the Rust backend when to
 *     attach the Python MCP sidecar.
 *
 * Everything else lives inside `<ChatView>`, which this component
 * customizes via its slot props.
 */
export default function App() {
  const session = useClaudeSession();
  const [dataset, setDataset] = useState<UploadedDataset | null>(null);
  const [uploadError, setUploadError] = useState<string | null>(null);

  const handleBeforeSend = async (_prompt: string) => {
    if (dataset) {
      await bindDataset(session.sessionId, dataset.dataset_id);
    }
  };

  const removeDataset = async () => {
    if (!dataset) return;
    await unbindDataset(session.sessionId);
    setDataset(null);
  };

  const handleReset = () => {
    // Starting a new chat also unbinds any dataset — clean slate.
    if (dataset) {
      unbindDataset(session.sessionId);
      setDataset(null);
    }
    setUploadError(null);
  };

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <ChatView
        session={session}
        onBeforeSend={handleBeforeSend}
        onReset={handleReset}
        composerLeftAdornment={
          <DatasetUploadButton
            onUpload={(d) => {
              setUploadError(null);
              setDataset(d);
            }}
            onError={(e) => setUploadError(e.message)}
            disabled={session.status === "running"}
          />
        }
        aboveComposer={
          <>
            {dataset && (
              <DatasetChip dataset={dataset} onRemove={removeDataset} />
            )}
            {uploadError && (
              <div className="border-t bg-destructive/10 px-3 py-2 text-xs text-destructive">
                {uploadError}
              </div>
            )}
          </>
        }
        renderEmptyState={({ onPick }) => (
          <EmptyState dataset={dataset} onPick={onPick} />
        )}
      />
    </div>
  );
}

function EmptyState({
  dataset,
  onPick,
}: {
  dataset: UploadedDataset | null;
  onPick: (prompt: string) => void;
}) {
  const ready = Boolean(dataset);
  const suggestions = ready
    ? [
        "Summarize this dataset — key columns, row count, and any obvious trends.",
        "Show the distribution of the most interesting numeric column.",
        "Plot the relationship between two columns that look correlated.",
      ]
    : [];

  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-6 py-8">
      <div className="space-y-1 text-center">
        <h1 className="text-2xl font-semibold tracking-tight">
          Agentic Visualization
        </h1>
        <p className="text-sm text-muted-foreground">
          Upload a CSV or JSON dataset, then ask a question. Claude replies
          with interleaved prose and charts in the same bubble.
        </p>
      </div>

      {!ready && (
        <div className="rounded-lg border border-dashed bg-card/40 p-6 text-center text-sm text-muted-foreground">
          <div className="mb-2 font-medium text-foreground">
            Start by uploading a dataset.
          </div>
          <div>
            Click <b>Dataset</b> in the composer below. CSV and JSON
            (array-of-objects) are supported, up to 25 MB.
          </div>
        </div>
      )}

      {ready && (
        <div className="rounded-lg border bg-card p-4 text-sm">
          <div className="mb-2 font-medium">{dataset!.filename} is ready.</div>
          <div className="text-muted-foreground">
            {dataset!.row_count.toLocaleString()} rows ·{" "}
            {dataset!.columns.length} columns. Try one of the prompts below
            or ask your own data question.
          </div>
        </div>
      )}

      <div className="rounded-lg border bg-card p-4 text-sm">
        <div className="mb-2 font-medium">What Claude can do</div>
        <ul className="list-disc space-y-1 pl-5 text-muted-foreground">
          <li>
            <code className="font-mono text-xs">describe_dataset</code> —
            schema, dtypes, and 5 sample rows.
          </li>
          <li>
            <code className="font-mono text-xs">query_dataset(sql)</code> —
            read-only SQL via DuckDB, capped at 500 rows.
          </li>
          <li>
            <code className="font-mono text-xs">create_chart(…)</code> —
            renders a Vega-Lite chart inline.
          </li>
        </ul>
      </div>

      {suggestions.length > 0 && (
        <div className="flex flex-col gap-2">
          <div className="text-center text-xs uppercase tracking-wide text-muted-foreground">
            Try a prompt
          </div>
          {suggestions.map((s) => (
            <button
              key={s}
              type="button"
              className="rounded border bg-card px-3 py-2 text-left text-sm hover:bg-muted"
              onClick={() => onPick(s)}
            >
              {s}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
