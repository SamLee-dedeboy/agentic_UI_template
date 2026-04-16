import { Button } from "@/components/ui/button";
import type { ClientToolProps } from "@/core/tools/registry";

/**
 * Reference client-tool renderer. Claude registers a `show_choice` tool
 * in the backend registry; when it calls the tool, the backend blocks
 * awaiting a UI response, the frontend routes the call here, this
 * component renders buttons, and the user's click resolves the call
 * with the chosen option's index.
 *
 * Forks typically replace this with domain-specific UI (a seat picker,
 * a date selector, a payment flow) and register under a matching name.
 */
interface ShowChoiceInput {
  prompt: string;
  options: string[];
}

type ShowChoiceResult = { index: number; value: string };

export function ShowChoice({
  input,
  resolve,
}: ClientToolProps<ShowChoiceInput, ShowChoiceResult>) {
  return (
    <div className="flex flex-col gap-2 rounded border bg-card p-3">
      <p className="text-sm">{input.prompt}</p>
      <div className="flex flex-col gap-1.5">
        {input.options.map((opt, i) => (
          <Button
            key={i}
            variant="outline"
            size="sm"
            className="justify-start"
            onClick={() => resolve({ index: i, value: opt })}
          >
            {opt}
          </Button>
        ))}
      </div>
    </div>
  );
}
