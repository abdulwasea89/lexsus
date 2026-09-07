import { useEffect, useState } from "react";
import {
  ProgressBar,
  ProgressBarFill,
  ProgressBarTrack,
  Spinner,
} from "@heroui/react";
import {
  ClipboardIcon,
  LightbulbIcon,
  MessageCircleIcon,
  RotateCcwIcon,
} from "lucide-react";
import { buildHandoff, handoffSend, setObjective } from "../lib/bridge";
import type { Handoff } from "../lib/types";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Label } from "../components/ui/label";
import { Separator } from "../components/ui/separator";
import { toast } from "../components/ui/toast";
import { ViewShell } from "./ViewShell";

/** Handoff view: build real state → "Continue with ChatGPT". Sends the
 *  payload to the paired extension (which renders it on chatgpt.com);
 *  falls back to clipboard copy. */
export default function HandoffView() {
  const [handoff, setHandoff] = useState<Handoff | null>(null);
  const [objective, setObj] = useState("");
  const [status, setStatus] = useState("");
  const [built, setBuilt] = useState(false);

  async function build() {
    const h = await buildHandoff();
    setHandoff(h);
    setObj(h.objective);
    setBuilt(true);
  }

  async function continueWithChatGPT() {
    if (!handoff) return;
    await setObjective(objective).catch(() => {});
    // handoffSend() rebuilds the payload from *current* state (the edited
    // objective, refreshed trace) and returns it — copy that, not the
    // `handoff` snapshot from the last build(), which is stale by the time
    // the user edits the objective and clicks send.
    const sent = await handoffSend();
    await navigator.clipboard.writeText(handoffText(sent)).catch(() => {});
    setStatus("handoff sent to the extension (also copied to clipboard)");
    toast.add({
      title: "Handoff sent",
      description: "Open chatgpt.com in the paired browser to continue.",
      type: "success",
    });
  }

  function handoffText(h: Handoff): string {
    const factBlock = [
      h.decisions?.length
        ? `Decisions made:\n${h.decisions.map((d) => `- ${d}`).join("\n")}`
        : "",
      h.failed_attempts?.length
        ? `Failed attempts (do not retry):\n${h.failed_attempts.map((a) => `- ${a}`).join("\n")}`
        : "",
      h.constraints?.length
        ? `Constraints:\n${h.constraints.map((c) => `- ${c}`).join("\n")}`
        : "",
    ]
      .filter(Boolean)
      .join("\n");
    return [
      `# Continue this task (AI Continuity Bridge handoff)`,
      ``,
      `Objective: ${h.objective}`,
      `Progress: ${h.progress_percent}% · Files changed: ${h.files_changed} · Errors remaining: ${h.errors_remaining}`,
      `Next step: ${h.next_step ?? "review state"}`,
      h.files.length > 0 ? `Files involved: ${h.files.join(", ")}` : "",
      factBlock ? `\n${factBlock}\n` : "",
      ``,
      `You are now the coding agent for the local project at the paired machine.`,
      `You may request file reads, file writes, and command runs; the bridge executes them locally and returns real results.`,
    ]
      .filter(Boolean)
      .join("\n");
  }

  useEffect(() => {
    void build();
  }, []);

  return (
    <ViewShell
      icon={MessageCircleIcon}
      title="Handoff → ChatGPT"
      description="built from real trace + watcher state"
      actions={
        handoff && (
          <Button
            variant="outline"
            size="sm"
            onClick={() =>
              void navigator.clipboard
                .writeText(handoffText(handoff))
                .then(() =>
                  toast.add({
                    title: "Copied",
                    description: "handoff text is on your clipboard",
                    type: "success",
                  }),
                )
            }
          >
            <ClipboardIcon /> Copy
          </Button>
        )
      }
    >
      {handoff ? (
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="objective">Objective</Label>
            <Input
              id="objective"
              value={objective}
              onChange={(e) => setObj(e.currentTarget.value)}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <div className="flex items-center justify-between text-xs">
              <span className="text-muted-foreground">Progress</span>
              <span className="font-mono">{handoff.progress_percent}%</span>
            </div>
            <ProgressBar
              value={handoff.progress_percent}
              color="accent"
              aria-label="Handoff progress"
            >
              <ProgressBarTrack className="h-1.5">
                <ProgressBarFill />
              </ProgressBarTrack>
            </ProgressBar>
          </div>

          <div className="grid grid-cols-3 gap-2">
            <div className="flex flex-col gap-0.5 rounded-lg border border-border/60 bg-surface-2/50 px-3 py-2">
              <span className="text-[11px] text-muted-foreground">
                Files changed
              </span>
              <span className="font-mono text-sm font-semibold">
                {handoff.files_changed}
              </span>
            </div>
            <div className="flex flex-col gap-0.5 rounded-lg border border-border/60 bg-surface-2/50 px-3 py-2">
              <span className="text-[11px] text-muted-foreground">
                Errors remaining
              </span>
              <span className="font-mono text-sm font-semibold">
                {handoff.errors_remaining}
              </span>
            </div>
            <div className="flex flex-col gap-0.5 rounded-lg border border-border/60 bg-surface-2/50 px-3 py-2">
              <span className="text-[11px] text-muted-foreground">State</span>
              <span className="font-mono text-sm font-semibold text-muted-foreground">
                {handoff.errors_remaining > 0 ? "in progress" : "on track"}
              </span>
            </div>
          </div>

          {handoff.next_step && (
            <p className="text-xs text-muted-foreground">
              next step: {handoff.next_step}
            </p>
          )}

          {handoff.end_reason && (
            <p className="text-[11px] text-muted-foreground">
              {handoff.end_reason}
            </p>
          )}

          {handoff.context && (
            <details className="rounded-lg border border-border/60 bg-surface-2/50 px-3 py-2">
              <summary className="cursor-pointer text-[11px] text-muted-foreground">
                task context (from your Claude Code transcript)
              </summary>
              <p className="mt-1 text-xs leading-relaxed text-foreground/80">
                {handoff.context}
              </p>
            </details>
          )}

          {((handoff.decisions?.length ?? 0) > 0 ||
            (handoff.failed_attempts?.length ?? 0) > 0 ||
            (handoff.constraints?.length ?? 0) > 0) && (
            <details className="rounded-lg border border-border/60 bg-surface-2/50 px-3 py-2">
              <summary className="cursor-pointer text-[11px] text-muted-foreground">
                extracted facts ({(handoff.decisions?.length ?? 0)} decisions,{" "}
                {(handoff.failed_attempts?.length ?? 0)} failed attempts,{" "}
                {(handoff.constraints?.length ?? 0)} constraints)
              </summary>
              <div className="mt-1 flex flex-col gap-1.5">
                {handoff.decisions?.map((d) => (
                  <p key={d} className="text-xs leading-relaxed text-info">
                    • {d}
                  </p>
                ))}
                {handoff.failed_attempts?.map((a) => (
                  <p key={a} className="text-xs leading-relaxed text-danger">
                    ✗ {a}
                  </p>
                ))}
                {handoff.constraints?.map((c) => (
                  <p key={c} className="text-xs leading-relaxed text-warning">
                    ⚠ {c}
                  </p>
                ))}
              </div>
            </details>
          )}

          <Separator />

          <div className="flex flex-wrap gap-2">
            <Button onClick={() => void continueWithChatGPT()}>
              <MessageCircleIcon /> Continue with ChatGPT
            </Button>
            <Button variant="ghost" onClick={() => void build()}>
              <RotateCcwIcon /> Rebuild
            </Button>
          </div>

          {status && <p className="text-xs text-success">{status}</p>}

          {built && !handoff.files_changed && (
            <p className="flex items-start gap-2 rounded-lg border border-border/60 bg-surface-2/50 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
              <LightbulbIcon className="mt-0.5 size-3.5 shrink-0" />
              Run a Claude Code session first — the card is built from what it
              did.
            </p>
          )}
        </div>
      ) : (
        <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
          <Spinner size="sm" /> building handoff…
        </div>
      )}
    </ViewShell>
  );
}
