import { useCallback, useEffect, useState } from "react";
import { Chip } from "@heroui/react";
import {
  CheckIcon,
  GitBranchIcon,
  HistoryIcon,
  ListTodoIcon,
  RefreshCwIcon,
  RotateCcwIcon,
} from "lucide-react";
import {
  gitBranches,
  gitCheckout,
  gitCommit,
  gitCommitDiff,
  gitDiff,
  gitLog,
  gitStage,
  gitStageAll,
  gitUnstage,
} from "../lib/bridge";
import type { BranchInfo, CommitInfo, FileDiff } from "../lib/types";
import { cn } from "../lib/utils";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { ScrollArea } from "../components/ui/scroll-area";
import { Separator } from "../components/ui/separator";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../components/ui/table";
import { Tabs, TabsList, TabsTrigger } from "../components/ui/tabs";
import { toast } from "../components/ui/toast";
import { ViewShell } from "./ViewShell";

type Tab = "status" | "branch" | "history";

type ChipColor = "accent" | "danger" | "default" | "success" | "warning";

const STATUS_COLOR: Record<string, ChipColor> = {
  added: "success",
  deleted: "danger",
  renamed: "accent",
  untracked: "default",
  conflicted: "danger",
};

/** Git view: status + stage/unstage + diff, branch switching, history,
 *  and commit — all via git2 in the Rust core. */
export default function GitView() {
  const [tab, setTab] = useState<Tab>("status");
  const [diffs, setDiffs] = useState<FileDiff[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [history, setHistory] = useState<CommitInfo[]>([]);
  const [commitDiff, setCommitDiff] = useState("");
  const [selectedCommit, setSelectedCommit] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [result, setResult] = useState("");
  const [error, setError] = useState("");

  const refresh = useCallback(async (opts?: { clearError?: boolean }) => {
    try {
      setDiffs(await gitDiff());
      setBranches(await gitBranches());
      setHistory(await gitLog(30));
      if (opts?.clearError !== false) setError("");
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** Run a git mutation, then refresh. On failure the error is surfaced
   *  (banner + toast) and the follow-up refresh must NOT clear it. */
  async function runGit(op: () => Promise<unknown>) {
    let failed: unknown = null;
    try {
      await op();
    } catch (e) {
      failed = e;
      setError(String(e));
      toast.add({
        title: "Git operation failed",
        description: String(e),
        type: "error",
      });
    }
    await refresh(failed ? { clearError: false } : undefined);
  }
  const stage = (path: string) => runGit(() => gitStage(path));
  const unstage = (path: string) => runGit(() => gitUnstage(path));
  const stageAll = () => runGit(() => gitStageAll());
  const checkout = (name: string) => runGit(() => gitCheckout(name));
  async function commit() {
    try {
      const oid = await gitCommit(message);
      setResult(`committed ${oid.slice(0, 8)}`);
      setMessage("");
      await refresh();
      toast.add({
        title: "Committed",
        description: `${oid.slice(0, 8)} recorded to the repository`,
        type: "success",
      });
    } catch (e) {
      setError(String(e));
      toast.add({ title: "Commit failed", description: String(e), type: "error" });
    }
  }
  async function showCommit(oid: string) {
    setSelectedCommit(oid);
    setCommitDiff(await gitCommitDiff(oid).catch((e) => `error: ${e}`));
  }

  const shown = diffs.filter((d) => !selected || d.path === selected);
  const selectedDiff = diffs.find((d) => d.path === selected);

  return (
    <ViewShell
      icon={GitBranchIcon}
      title="Git"
      description={error ? undefined : "status · branch · history"}
      padded={false}
      actions={
        <Tabs value={tab} onValueChange={(v) => setTab(v as Tab)}>
          <TabsList>
            <TabsTrigger value="status" className="gap-1">
              <ListTodoIcon /> Status
            </TabsTrigger>
            <TabsTrigger value="branch" className="gap-1">
              <GitBranchIcon /> Branch
            </TabsTrigger>
            <TabsTrigger value="history" className="gap-1">
              <HistoryIcon /> History
            </TabsTrigger>
          </TabsList>
        </Tabs>
      }
    >
      {error && (
        <p className="shrink-0 border-b border-border bg-danger/10 px-4 py-2 text-xs text-danger">
          {error}
        </p>
      )}

      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-4">
        {tab === "status" && (
          <>
            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" onClick={() => void stageAll()}>
                Stage all
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={() => void refresh()}
                aria-label="Refresh"
              >
                <RefreshCwIcon /> Refresh
              </Button>
              {selected && (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => setSelected(null)}
                >
                  <RotateCcwIcon /> Show all files
                </Button>
              )}
              {result && <Badge variant="secondary">{result}</Badge>}
            </div>

            {diffs.length === 0 ? (
              <div className="flex flex-1 items-center justify-center p-8 text-center">
                <div className="flex flex-col items-center gap-2">
                  <CheckIcon className="size-8 text-muted-foreground/50" />
                  <p className="text-sm font-medium">Working tree clean</p>
                  <p className="text-xs text-muted-foreground">
                    No changes to show — edit a file to see it here.
                  </p>
                </div>
              </div>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>File</TableHead>
                    <TableHead>Status</TableHead>
                    <TableHead className="text-right">+/-</TableHead>
                    <TableHead className="w-32 text-right">Actions</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {shown.map((d) => (
                    <TableRow
                      key={d.path}
                      onClick={() => setSelected(d.path)}
                      className={cn(
                        "cursor-pointer",
                        selected === d.path && "bg-muted/60",
                      )}
                    >
                      <TableCell className="max-w-0 font-mono text-xs">
                        <span className="block truncate">{d.path}</span>
                      </TableCell>
                      <TableCell>
                        <Chip
                          size="sm"
                          variant="soft"
                          color={STATUS_COLOR[d.status] ?? "warning"}
                        >
                          {d.status}
                        </Chip>
                      </TableCell>
                      <TableCell className="text-right font-mono text-xs whitespace-nowrap">
                        <span className="text-success">+{d.added}</span>{" "}
                        <span className="text-danger">-{d.deleted}</span>
                      </TableCell>
                      <TableCell className="text-right">
                        <Button
                          size="xs"
                          variant="ghost"
                          onClick={(e) => {
                            e.stopPropagation();
                            void stage(d.path);
                          }}
                        >
                          Stage
                        </Button>
                        <Button
                          size="xs"
                          variant="ghost"
                          onClick={(e) => {
                            e.stopPropagation();
                            void unstage(d.path);
                          }}
                        >
                          Unstage
                        </Button>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}

            {selectedDiff && (
              <ScrollArea className="h-36 shrink-0 rounded-lg border border-border/60 bg-surface-2/50 p-3">
                <pre className="font-mono text-[11px] leading-relaxed text-muted-foreground">
                  {selectedDiff.patch}
                </pre>
              </ScrollArea>
            )}

            <Separator />

            <div className="flex gap-2">
              <Input
                value={message}
                placeholder="Commit message"
                onChange={(e) => setMessage(e.currentTarget.value)}
                onKeyDown={(e) => e.key === "Enter" && message.trim() && commit()}
              />
              <Button onClick={() => void commit()} disabled={!message.trim()}>
                Commit
              </Button>
            </div>
          </>
        )}

        {tab === "branch" &&
          (branches.length === 0 ? (
            <div className="flex flex-1 items-center justify-center p-8 text-center">
              <div className="flex flex-col items-center gap-2">
                <GitBranchIcon className="size-8 text-muted-foreground/50" />
                <p className="text-sm font-medium">No branches</p>
                <p className="text-xs text-muted-foreground">
                  Open a project folder to inspect its branches.
                </p>
              </div>
            </div>
          ) : (
            <ul className="flex flex-col gap-1">
              {branches.map((b) => (
                <li
                  key={b.name}
                  className="flex items-center justify-between gap-2 rounded-md px-2 py-1.5 hover:bg-muted/40"
                >
                  <span className="flex min-w-0 items-center gap-2 font-mono text-xs">
                    <GitBranchIcon className="size-3.5 shrink-0 text-muted-foreground" />
                    <span className="truncate">{b.name}</span>
                  </span>
                  {b.is_current ? (
                    <Chip size="sm" variant="soft" color="success">
                      current
                    </Chip>
                  ) : (
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => void checkout(b.name)}
                    >
                      Checkout
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          ))}

        {tab === "history" && (
          <div className="flex min-h-0 flex-1 flex-col gap-3">
            {history.length === 0 ? (
              <div className="flex flex-1 items-center justify-center p-8 text-center">
                <div className="flex flex-col items-center gap-2">
                  <HistoryIcon className="size-8 text-muted-foreground/50" />
                  <p className="text-sm font-medium">No history</p>
                  <p className="text-xs text-muted-foreground">
                    Commits made in this repository will appear here.
                  </p>
                </div>
              </div>
            ) : (
              <ScrollArea className="min-h-0 flex-1 pr-3">
                <ul className="flex flex-col gap-0.5">
                  {history.map((c) => (
                    <li key={c.oid}>
                      <Button
                        variant="ghost"
                        className={cn(
                          "h-auto w-full justify-start px-2 py-1.5",
                          selectedCommit === c.oid && "bg-muted/60",
                        )}
                        onClick={() => void showCommit(c.oid)}
                      >
                        <span className="flex flex-col items-start gap-0.5">
                          <span className="text-xs font-medium">
                            {c.message}
                          </span>
                          <span className="text-[11px] text-muted-foreground">
                            {c.oid.slice(0, 7)} · {c.author} ·{" "}
                            {new Date(c.timestamp * 1000).toLocaleString()}
                          </span>
                        </span>
                      </Button>
                    </li>
                  ))}
                </ul>
              </ScrollArea>
            )}
            {commitDiff && (
              <ScrollArea className="h-40 shrink-0 rounded-lg border border-border/60 bg-surface-2/50 p-3">
                <pre className="font-mono text-[11px] leading-relaxed text-muted-foreground">
                  {commitDiff}
                </pre>
              </ScrollArea>
            )}
          </div>
        )}
      </div>
    </ViewShell>
  );
}
