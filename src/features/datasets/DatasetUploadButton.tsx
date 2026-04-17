import { useRef, useState } from "react";
import { Paperclip, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { uploadDataset } from "./api";
import type { UploadedDataset } from "./types";

/**
 * Composer-embedded upload button. Accepts CSV and JSON; on success,
 * calls `onUpload` with the server-side dataset_id + schema so the
 * parent can render a chip and bind on the next send.
 */
export function DatasetUploadButton({
  onUpload,
  onError,
  disabled,
}: {
  onUpload: (dataset: UploadedDataset) => void;
  onError?: (err: Error) => void;
  disabled?: boolean;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [uploading, setUploading] = useState(false);

  const pick = () => {
    if (disabled || uploading) return;
    inputRef.current?.click();
  };

  const onChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    // Reset so selecting the same file twice still fires change.
    if (inputRef.current) inputRef.current.value = "";
    if (!file) return;
    setUploading(true);
    try {
      const dataset = await uploadDataset(file);
      onUpload(dataset);
    } catch (err) {
      onError?.(err instanceof Error ? err : new Error(String(err)));
    } finally {
      setUploading(false);
    }
  };

  return (
    <>
      <input
        ref={inputRef}
        type="file"
        accept=".csv,.tsv,.json,text/csv,application/json"
        className="hidden"
        onChange={onChange}
      />
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={pick}
        disabled={disabled || uploading}
        title="Upload a CSV or JSON dataset"
      >
        {uploading ? (
          <Loader2 className="h-4 w-4 animate-spin" />
        ) : (
          <Paperclip className="h-4 w-4" />
        )}
        <span className="ml-1.5">Dataset</span>
      </Button>
    </>
  );
}
