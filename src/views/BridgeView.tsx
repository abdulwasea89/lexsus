import { useEffect, useState } from "react";
import {
  ChevronDownIcon,
  CopyIcon,
  EyeIcon,
  EyeOffIcon,
  GlobeIcon,
  RefreshCwIcon,
} from "lucide-react";
import {
  bridgeAudit,
  bridgeTool,
  mcpRevealToken,
  mcpRotateToken,
  mcpSetAllowWrite,
  mcpStatus,
} from "../lib/bridge";
import type {
  AuditEntry,
  BridgeTool,
  McpStatus,
  ToolResult,
} from "../lib/types";
import { cn } from "../lib/utils";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../components/ui/collapsible";
import { Input } from "../components/ui/input";
import { Label } from "../components/ui/label";
import { ScrollArea } from "../components/ui/scroll-area";
import { Switch } from "../components/ui/switch";
import { toast } from "../components/ui/toast";
import { ViewShell } from "./ViewShell";

/**
 * Web-AI bridge view: the connector's live state (endpoint, bound
 * workspace, the read-only/write switch, the bearer credential), a tool
 * sandbox for testing read/write/run locally, and the audit trail.
 * Approval requests live in the global banner — this view is diagnostics
 * only.
 */
export default function BridgeView() {
  const [audit, setAudit] = useState<AuditEntry[]>([]);
  const [connector, setConnector] = useState<McpStatus | null>(null);
  const [readPath, setReadPath] = useState("src/App.tsx");
  const [writePath, setWritePath] = useState("");
  const [writeContent, setWriteContent] = useState("");
  const [command, setCommand] = useState("git status");
  const [sandbox, setSandbox] = useState<ToolResult | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    void bridgeAudit(20)
      .then(setAudit)
      .catch(() => []);
    void mcpStatus()
      .then(setConnector)
      .catch(() => null);
  }, []);

  async function sandboxRun(tool: BridgeTool) {
    setSandbox(await bridgeTool(tool));
    setAudit(await bridgeAudit(20).catch(() => []));
  }

  /**
   * Fetch the token on reveal, not on mount: it is a live credential, and
   * holding it in view state for the life of the tab buys nothing.
   */
  async function toggleReveal() {
    if (revealed) {
      setRevealed(false);
      return;
    }
    try {
      setToken(await mcpRevealToken());
      setRevealed(true);
    } catch (e) {
      toast.add({
        title: "Could not read the token",
        description: String(e),
        type: "error",
      });
    }
  }

  function copy(text: string, what: string) {
    void navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
        toast.add({ title: "Copied", description: what, type: "success" });
      })
      .catch(() => {});
  }

  /**
   * Rotate, then drop the cached token: the revealed value is now dead, and
   * leaving it on screen would invite pasting it into a new connector.
   */
  async function rotate() {
    try {
      setConnector(await mcpRotateToken());
      setToken(null);
      setRevealed(false);
      toast.add({
        title: "Token rotated",
        description: "the previous token now gets 401",
        type: "success",
      });
    } catch (e) {
      toast.add({
        title: "Rotation failed",
        description: String(e),
        type: "error",
      });
    }
  }

  const backendNote =
    connector?.auth_backend === "file"
      ? "Stored in a 0600 file in the app data dir — the OS keyring was unavailable (common on headless Linux)."
      : connector?.auth_backend === "keyring"
        ? "Stored in your OS keyring."
        : "Taken from LEXSUS_MCP_AUTH_TOKEN, which overrides the keyring and the file.";

  const claudeCodeSnippet = connector
    ? `claude mcp add --transport http lexsus ${connector.endpoint} \\\n  --header "Authorization: Bearer ${
        token ?? "<reveal the token>"
      }"`
    : "";

  const sectionClass =
    "flex w-full items-center justify-between gap-2 rounded-lg border border-border/60 bg-surface-2/50 px-3 py-2 text-xs font-medium text-muted-foreground hover:text-foreground";

  return (
    <ViewShell
      icon={GlobeIcon}
      title="Web-AI connector"
      description={`MCP endpoint · tool sandbox · audit trail (last ${audit.length})`}
    >
      <div className="flex flex-col gap-3">
        <Collapsible className="flex flex-col gap-2" defaultOpen>
          <CollapsibleTrigger className={sectionClass}>
            MCP connector
            <ChevronDownIcon className="size-4 transition-transform data-[state=open]:rotate-180" />
          </CollapsibleTrigger>
          <CollapsibleContent className="data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0">
            <div className="flex flex-col gap-3 rounded-lg border border-border/60 bg-surface-2/50 p-3">
              <div className="flex items-center justify-between gap-3">
                <span className="shrink-0 text-[11px] text-muted-foreground">
                  Endpoint
                </span>
                <code
                  className="truncate font-mono text-xs"
                  title={connector?.endpoint}
                >
                  {connector?.endpoint ?? "—"}
                </code>
              </div>
              <div className="flex items-center justify-between gap-3">
                <span className="shrink-0 text-[11px] text-muted-foreground">
                  Workspace
                </span>
                <span
                  className="truncate font-mono text-xs"
                  title={connector?.workspace ?? ""}
                >
                  {connector?.workspace ?? "no project bound"}
                </span>
              </div>
              <div className="flex items-center justify-between gap-3">
                <div className="flex min-w-0 flex-col">
                  <span className="shrink-0 text-[11px] text-muted-foreground">
                    Allowed hosts
                  </span>
                  <span className="text-[11px] leading-relaxed text-muted-foreground">
                    A tunnel host must be listed here (via
                    LEXSUS_MCP_ALLOWED_HOSTS), or its requests get a 403 that
                    reads as a sign-in failure.
                  </span>
                </div>
                <code
                  className="shrink-0 truncate font-mono text-xs"
                  title={connector?.allowed_hosts.join(", ")}
                >
                  {connector?.allowed_hosts.join(", ") || "—"}
                </code>
              </div>
              <div className="flex items-center justify-between gap-3">
                <div className="flex min-w-0 flex-col">
                  <span className="text-xs font-medium">
                    Allow writes &amp; commands
                  </span>
                  <span className="text-[11px] leading-relaxed text-muted-foreground">
                    Off = read-only surface. Every write still needs your
                    approval here, on the desktop.
                  </span>
                </div>
                <Switch
                  size="sm"
                  checked={connector?.allow_write ?? false}
                  onCheckedChange={(checked) => {
                    void mcpSetAllowWrite(checked).then(setConnector);
                  }}
                />
              </div>
            </div>
          </CollapsibleContent>
        </Collapsible>

        <Collapsible className="flex flex-col gap-2">
          <CollapsibleTrigger className={sectionClass}>
            Connector authentication
            <span className="flex items-center gap-2">
              <Badge variant="outline" className="font-mono text-[10px]">
                {connector?.auth_backend ?? "—"}
              </Badge>
              <ChevronDownIcon className="size-4 transition-transform data-[state=open]:rotate-180" />
            </span>
          </CollapsibleTrigger>
          <CollapsibleContent className="data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0">
            <div className="flex flex-col gap-3 rounded-lg border border-border/60 bg-surface-2/50 p-3">
              <p className="text-[11px] leading-relaxed text-muted-foreground">
                Every request to the endpoint must carry{" "}
                <code className="font-mono">Authorization: Bearer …</code>, on
                loopback too. Responses are signed. A signature on the request
                is verified when present, but is not required — no MCP client
                can produce one.
              </p>

              <div className="flex items-center justify-between gap-3">
                <span className="shrink-0 text-[11px] text-muted-foreground">
                  Token
                </span>
                <div className="flex min-w-0 flex-1 items-center justify-end gap-2">
                  <code
                    className="min-w-0 flex-1 truncate text-right font-mono text-xs"
                    title={revealed && token ? token : undefined}
                  >
                    {revealed && token ? token : "•".repeat(40)}
                  </code>
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    className="shrink-0"
                    title={revealed ? "Hide" : "Reveal"}
                    onClick={() => void toggleReveal()}
                  >
                    {revealed ? (
                      <EyeOffIcon className="size-4" />
                    ) : (
                      <EyeIcon className="size-4" />
                    )}
                  </Button>
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    className="shrink-0"
                    title="Copy the token"
                    disabled={!revealed || !token}
                    onClick={() => token && copy(token, "token is on your clipboard")}
                  >
                    <CopyIcon className="size-4" />
                  </Button>
                </div>
              </div>

              <div className="flex items-center justify-between gap-3">
                <div className="flex min-w-0 flex-col">
                  <span className="shrink-0 text-[11px] text-muted-foreground">
                    Fingerprint
                  </span>
                  <span className="text-[11px] leading-relaxed text-muted-foreground">
                    Which token is live, without showing it.
                  </span>
                </div>
                <code className="shrink-0 font-mono text-xs">
                  {connector?.token_fingerprint ?? "—"}
                </code>
              </div>

              <p className="text-[11px] leading-relaxed text-muted-foreground">
                {backendNote}
              </p>

              <div className="flex flex-wrap items-center gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void rotate()}
                >
                  <RefreshCwIcon className="size-3.5" />
                  Rotate token
                </Button>
                <span className="text-[11px] text-muted-foreground">
                  The old token stops working at once, with no restart.
                </span>
              </div>

              <div className="flex flex-col gap-2 rounded-md border border-border/60 bg-background/60 p-3">
                <div className="flex items-center justify-between gap-3">
                  <span className="text-[11px] font-medium">Claude Code</span>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      copy(claudeCodeSnippet, "command is on your clipboard")
                    }
                  >
                    <CopyIcon className="size-3.5" />
                    {copied ? "Copied" : "Copy"}
                  </Button>
                </div>
                <pre className="overflow-x-auto whitespace-pre-wrap break-all font-mono text-[11px] leading-relaxed text-muted-foreground">
                  {claudeCodeSnippet || "—"}
                </pre>
                <p className="text-[11px] leading-relaxed text-muted-foreground">
                  MCP Inspector and any other client take the same header. For
                  a tunnel to Claude.ai, the edge (cloudflared or LocalCan) must
                  inject{" "}
                  <code className="font-mono">Authorization: Bearer …</code>{" "}
                  itself — custom connectors cannot send a header, which is why
                  OAuth 2.1 + PKCE is the real fix there.
                </p>
              </div>

              <p className="text-[11px] leading-relaxed text-muted-foreground">
                Signature TTL {connector?.signature_ttl_secs ?? "—"}s · signed
                requests{" "}
                {connector?.signature_required ? "required" : "optional"}
              </p>
            </div>
          </CollapsibleContent>
        </Collapsible>

        <Collapsible className="flex flex-col gap-2">
          <CollapsibleTrigger className={sectionClass}>
            Tool sandbox (test read / write / run locally)
            <ChevronDownIcon className="size-4 transition-transform data-[state=open]:rotate-180" />
          </CollapsibleTrigger>
          <CollapsibleContent className="data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0">
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
            <ChevronDownIcon className="size-4 transition-transform data-[state=open]:rotate-180" />
          </CollapsibleTrigger>
          <CollapsibleContent className="data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0">
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
