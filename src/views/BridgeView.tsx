import { useEffect, useState } from "react";
import { ChevronDownIcon, GlobeIcon } from "lucide-react";
import { bridgeAudit, bridgeTool } from "../lib/bridge";
import type { AuditEntry, BridgeTool, ToolResult } from "../lib/types";
import { cn } from "../lib/utils";
import { Button } from "../components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
import { Input } from "../components/ui/input";
import { Label } from "../components/ui/label";
import { ScrollArea } from "../components/ui/scroll-area";
import { ViewShell } from "./ViewShell";

/**
 * Web-AI bridge view: a tool sandbox for testing read/write/run locally
 * and the audit trail. Approval requests live in the global banner —
 * this view is diagnostics only.
 */
export default function BridgeView() {
  const [audit, setAudit] = useState<AuditEntry[]>([]);
  const [readPath, setReadPath] = useState("src/App.tsx");
  const [writePath, setWritePath] = useState("");
  const [writeContent, setWriteContent] = useState("");
  const [command, setCommand] = useState("git status");
  const [sandbox, setSandbox] = useState<ToolResult | null>(null);

  useEffect(() => {
    void bridgeAudit(20)
      .then(setAudit)
      .catch(() => []);
  }, []);

  async function sandboxRun(tool: BridgeTool) {
    setSandbox(await bridgeTool(tool));
    setAudit(await bridgeAudit(20).catch(() => []));
  }

  const sectionClass =
    "flex w-full items-center justify-between gap-2 rounded-lg border border-border/60 bg-surface-2/50 px-3 py-2 text-xs font-medium text-muted-foreground hover:text-foreground";

  return (
    <ViewShell
      icon={GlobeIcon}
      title="Web-AI bridge"
      description={`tool sandbox · audit trail (last ${audit.length})`}
    >
      <div className="flex flex-col gap-3">
        <Collapsible className="flex flex-col gap-2">
          <CollapsibleTrigger className={sectionClass}>
            Tool sandbox (test read / write / run locally)
            <ChevronDownIcon className="size-4" />
          </CollapsibleTrigger>
          <CollapsibleContent>
            <div className="flex min-h-0 flex-col gap-3 rounded-lg border border-border/60 bg-surface-2/50 p-3">
              <div className="flex flex-col gap-2 sm:flex-row sm:items-end sm:gap-3">
                <Label className="shrink-0 text-[11px] text-muted-foreground sm:w-28 sm:pb-2">
                  read_file
                </Label>
                <div className="flex min-w-0 flex-1 items-end gap-2">
                  <Input
                    value={readPath}
                    onChange={(e) => setReadPath(e.currentTarget.value)}
                    className="min-w-0 flex-1 font-mono text-xs"
                  />
                  <Button
                    size="sm"
                    variant="outline"
                    className="shrink-0"
                    onClick={() =>
                      void sandboxRun({ ReadFile: { path: readPath } })
                    }
                  >
                    Read
                  </Button>
                </div>
              </div>
              <div className="flex flex-col gap-2 sm:flex-row sm:items-end sm:gap-3">
                <Label className="shrink-0 text-[11px] text-muted-foreground sm:w-28 sm:pb-2">
                  write_file
                </Label>
                <div className="flex min-w-0 flex-1 flex-col gap-2 sm:flex-row sm:items-end">
                  <Input
                    value={writePath}
                    placeholder="path"
                    onChange={(e) => setWritePath(e.currentTarget.value)}
                    className="min-w-0 flex-1 font-mono text-xs"
                  />
                  <Input
                    value={writeContent}
                    placeholder="content"
                    onChange={(e) => setWriteContent(e.currentTarget.value)}
                    className="min-w-0 flex-[2] font-mono text-xs"
                  />
                  <Button
                    size="sm"
                    variant="outline"
                    className="shrink-0"
                    onClick={() =>
                      void sandboxRun({
                        WriteFile: { path: writePath, content: writeContent },
                      })
                    }
                  >
                    Write
                  </Button>
                </div>
              </div>
              <div className="flex flex-col gap-2 sm:flex-row sm:items-end sm:gap-3">
                <Label className="shrink-0 text-[11px] text-muted-foreground sm:w-28 sm:pb-2">
                  run_command
                </Label>
                <div className="flex min-w-0 flex-1 items-end gap-2">
                  <Input
                    value={command}
                    onChange={(e) => setCommand(e.currentTarget.value)}
                    className="min-w-0 flex-1 font-mono text-xs"
                  />
                  <Button
                    size="sm"
                    variant="outline"
                    className="shrink-0"
                    onClick={() =>
                      void sandboxRun({ RunCommand: { command } })
                    }
                  >
                    Run
                  </Button>
                </div>
              </div>
              {sandbox && (
                <ScrollArea className="h-28 min-h-0 rounded-md border border-border/60 bg-background/60 p-3">
                  <pre
                    className={cn(
                      "whitespace-pre-wrap break-words font-mono text-[11px] leading-relaxed",
                      sandbox.ok ? "text-foreground" : "text-danger",
                    )}
                  >
                    {sandbox.ok
                      ? sandbox.output
                      : sandbox.error ?? sandbox.pending ?? "?"}
                  </pre>
                </ScrollArea>
              )}
            </div>
          </CollapsibleContent>
        </Collapsible>

        <Collapsible className="flex flex-col gap-2">
          <CollapsibleTrigger className={sectionClass}>
            Audit trail (last {audit.length})
            <ChevronDownIcon className="size-4" />
          </CollapsibleTrigger>
          <CollapsibleContent>
            <ScrollArea className="h-40 min-h-0 rounded-lg border border-border/60 bg-surface-2/50 p-3">
              {audit.length === 0 ? (
                <p className="text-xs text-muted-foreground">
                  No tool calls recorded yet.
                </p>
              ) : (
                <ul className="flex flex-col gap-1">
                  {audit.map((a, i) => (
                    <li
                      key={i}
                      className={cn(
                        "flex gap-2 font-mono text-[11px]",
                        !a.allowed && "text-danger",
                      )}
                    >
                      <span className="shrink-0 text-muted-foreground">
                        [{a.ts}]
                      </span>
                      <span className="min-w-0 flex-1 whitespace-pre-wrap break-words">
                        {a.agent} · {a.tool} · {a.args} ·{" "}
                        {a.allowed
                          ? `allowed (${a.approved_by})`
                          : "DENIED"}{" "}
                        · {a.ok ? "ok" : "failed"}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </ScrollArea>
          </CollapsibleContent>
        </Collapsible>
      </div>
    </ViewShell>
  );
}
