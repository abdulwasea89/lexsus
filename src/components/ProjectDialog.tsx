import { Chip } from "@heroui/react";
import { FolderIcon, FolderOpenIcon } from "lucide-react";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Separator } from "./ui/separator";
import type { McpStatus } from "../lib/types";

interface ProjectDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectRoot: string;
  recents: string[];
  connector: McpStatus | null;
  onPick: (path: string) => void;
  onBrowse: () => void;
}

/**
 * Project & connector setup, moved off the old sidebar into one dialog:
 * recent folders, browse, and the loopback MCP endpoint a web AI's
 * connector points at. Opened from the workbench rail.
 */
export default function ProjectDialog({
  open,
  onOpenChange,
  projectRoot,
  recents,
  connector,
  onPick,
  onBrowse,
}: ProjectDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Project &amp; connector</DialogTitle>
          <DialogDescription>
            Pick the folder the web AI works on, then point a connector at
            the local endpoint below.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <p className="app-eyebrow text-muted-foreground">Working directory</p>
            {projectRoot && (
              <p
                className="truncate rounded-md border border-border bg-muted/40 px-2.5 py-1.5 font-mono text-xs"
                title={projectRoot}
              >
                {projectRoot}
              </p>
            )}
            <div className="mt-1 flex flex-col gap-0.5">
              {recents.length === 0 && !projectRoot && (
                <p className="px-1 text-xs leading-relaxed text-muted-foreground">
                  No folders yet — browse to pick your first project.
                </p>
              )}
              {recents.map((p) => (
                <Button
                  key={p}
                  variant="ghost"
                  size="sm"
                  className="justify-start gap-2 font-mono text-xs"
                  onClick={() => {
                    onPick(p);
                    onOpenChange(false);
                  }}
                >
                  <FolderIcon className="shrink-0 text-muted-foreground" />
                  <span className="truncate" title={p}>
                    {p}
                  </span>
                </Button>
              ))}
            </div>
            <Button
              variant="outline"
              size="sm"
              className="mt-1 justify-start"
              onClick={() => {
                onBrowse();
                onOpenChange(false);
              }}
            >
              <FolderOpenIcon /> Browse folder…
            </Button>
          </div>

          <Separator />

          <div className="flex flex-col gap-2">
            <p className="app-eyebrow text-muted-foreground">Connector</p>
            <div className="flex items-center justify-between gap-2">
              <Chip
                color={connector?.listening ? "success" : "default"}
                variant="soft"
                size="sm"
              >
                {connector?.listening ? "Listening" : "Offline"}
              </Chip>
              {connector ? (
                <code
                  className="truncate rounded-md border border-border bg-muted/40 px-2.5 py-1 font-mono text-xs"
                  title={connector.endpoint}
                >
                  {connector.endpoint}
                </code>
              ) : (
                <span className="text-xs text-muted-foreground">
                  endpoint appears after the server starts
                </span>
              )}
            </div>
            <p className="text-[11px] leading-relaxed text-muted-foreground">
              Point a remote-MCP connector (Claude.ai → Customize →
              Connectors) or a local MCP host at this URL. It binds loopback
              only — a hosted AI reaches it through a tunnel you run. The
              surface is read-only until you enable writes in the bridge view.
            </p>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
