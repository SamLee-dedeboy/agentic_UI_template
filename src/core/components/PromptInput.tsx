import { useState, type KeyboardEvent } from "react";
import { Button } from "@/components/ui/button";

/**
 * Tiny prompt composer. Enter submits; Shift+Enter inserts a newline.
 * Deliberately bare — forks add slash-command pickers, file pickers, model
 * selectors, etc. by wrapping or replacing this component.
 */
export function PromptInput({
  onSubmit,
  onCancel,
  disabled,
  running,
}: {
  onSubmit: (prompt: string) => void;
  onCancel?: () => void;
  disabled?: boolean;
  running?: boolean;
}) {
  const [value, setValue] = useState("");

  const submit = () => {
    const trimmed = value.trim();
    if (!trimmed || disabled) return;
    onSubmit(trimmed);
    setValue("");
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  return (
    <div className="flex items-end gap-2 border-t p-3">
      <textarea
        className="flex-1 resize-none rounded border bg-background p-2 text-sm outline-none focus:ring-1 focus:ring-ring"
        placeholder={running ? "Streaming… (Shift+Enter for newline)" : "Ask Claude something… (Enter to send)"}
        rows={3}
        value={value}
        disabled={disabled}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKey}
      />
      {running && onCancel ? (
        <Button variant="destructive" onClick={onCancel}>
          Cancel
        </Button>
      ) : (
        <Button onClick={submit} disabled={disabled || !value.trim()}>
          Send
        </Button>
      )}
    </div>
  );
}
