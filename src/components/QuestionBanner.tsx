import { useState } from "react";
import { HelpCircleIcon, ListChecksIcon } from "lucide-react";
import type { QuestionCard } from "../hooks/useQuestions";
import { Button } from "./ui/button";

interface QuestionBannerProps {
  questions: QuestionCard[];
  onAnswer: (id: number, option: string | null, text: string) => void;
}

/**
 * The agent-loop card. A question shows its labelled options plus a free-text
 * box; a plan shows the plan and an Allow / Deny pair. Either way the web
 * AI's call is blocked until this is answered — that is the point, and it is
 * why the card says so.
 */
export default function QuestionBanner({
  questions,
  onAnswer,
}: QuestionBannerProps) {
  const [freeText, setFreeText] = useState<Record<number, string>>({});

  if (questions.length === 0) return null;

  return (
    <div className="flex shrink-0 flex-col gap-2 border-b border-primary/30 bg-primary/10 px-4 py-2.5">
      {questions.map((q) => {
        const text = freeText[q.id] ?? "";
        const isPlan = q.kind === "plan";
        return (
          <div key={q.id} className="flex flex-col gap-2" role="alert">
            <div className="flex items-center gap-2.5">
              {isPlan ? (
                <ListChecksIcon className="size-4 shrink-0 text-primary" />
              ) : (
                <HelpCircleIcon className="size-4 shrink-0 text-primary" />
              )}
              <p className="min-w-0 flex-1 text-sm">
                <span className="font-semibold text-primary">
                  {q.source === "mcp" ? "Web AI" : "Desktop"}{" "}
                  {isPlan ? "proposes a plan:" : "asks:"}
                </span>{" "}
                <span className="font-medium">{q.title}</span>
              </p>
            </div>
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-md bg-background/60 p-2 text-xs">
              {q.body}
            </pre>
            <div className="flex flex-wrap items-center gap-1.5">
              {q.options.map((option) => (
                <Button
                  key={option}
                  size="sm"
                  variant={isPlan && option === "Allow" ? "default" : "secondary"}
                  onClick={() => onAnswer(q.id, option, text)}
                >
                  {option}
                </Button>
              ))}
              <input
                className="h-8 min-w-40 flex-1 rounded-md border border-input bg-background px-2 text-sm"
                placeholder={isPlan ? "Optional comment…" : "Or type an answer…"}
                value={text}
                onChange={(e) =>
                  setFreeText((prev) => ({ ...prev, [q.id]: e.target.value }))
                }
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !isPlan) {
                    onAnswer(q.id, null, text);
                  }
                }}
              />
              {!isPlan && (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={text.trim().length === 0}
                  onClick={() => onAnswer(q.id, null, text)}
                >
                  Submit
                </Button>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}
