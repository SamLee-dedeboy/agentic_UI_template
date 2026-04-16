import { Check } from "lucide-react";
import type { ToolResultProps } from "@/core/tools/registry";

/**
 * Result renderer for the `show_choice` client tool. Paired with
 * `ShowChoice.tsx`, which resolves with `{ index, value }` when the
 * user clicks a button. This card confirms the user's choice back in
 * the chat so Claude's next turn has a visible anchor to refer to.
 */
interface ChoiceResult {
  index?: number;
  value?: string;
}

export function ChoiceResultCard({ content }: ToolResultProps<ChoiceResult>) {
  const value = content?.value ?? "(unknown)";
  return (
    <div className="flex items-center gap-2 rounded-full border bg-card px-3 py-1.5 text-sm">
      <Check className="h-3.5 w-3.5 text-green-600" />
      <span className="text-muted-foreground">You picked</span>
      <span className="font-medium">{value}</span>
    </div>
  );
}
