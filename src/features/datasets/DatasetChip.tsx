import { Database, X } from "lucide-react";
import type { UploadedDataset } from "./types";

/**
 * Small pill shown above the composer once a dataset has been uploaded.
 * Reminds the user which file Claude is reasoning over and lets them
 * remove it (which also unbinds on the backend and — by convention in
 * App.tsx — clears the current chat).
 */
export function DatasetChip({
  dataset,
  onRemove,
}: {
  dataset: UploadedDataset;
  onRemove: () => void;
}) {
  return (
    <div className="flex items-center gap-2 border-t bg-muted/40 px-3 py-2 text-xs">
      <Database className="h-4 w-4 shrink-0 text-muted-foreground" />
      <span className="truncate font-medium">{dataset.filename}</span>
      <span className="text-muted-foreground">
        · {dataset.row_count.toLocaleString()} rows · {dataset.columns.length} cols
      </span>
      <button
        type="button"
        onClick={onRemove}
        className="ml-auto flex items-center gap-1 rounded px-1.5 py-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
        aria-label="Remove dataset"
      >
        <X className="h-3.5 w-3.5" />
        Remove
      </button>
    </div>
  );
}
