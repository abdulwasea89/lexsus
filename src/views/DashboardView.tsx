import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import {
  ActivityIcon,
  BookOpenIcon,
  BoxesIcon,
  CheckIcon,
  ChevronDownIcon,
  CircleAlertIcon,
  CopyIcon,
  FileIcon,
  FolderOpenIcon,
  GlobeIcon,
  Loader2Icon,
  RefreshCwIcon,
  ShieldAlertIcon,
  SquareIcon,
  SquarePenIcon,
  SquareTerminalIcon,
  XIcon,
} from "lucide-react";
import {
  activityCommands,
  activityFiles,
  activityStats,
  activityToolSurface,
  activityToolUsage,
  activityTrace,
  bridgeAudit,
  mcpRestart,
  mcpSetAllowWrite,
  mcpSetAllowedHosts,
  mcpSetPort,
  mcpStart,
  mcpStatus,
  mcpStop,
  tunnelDetect,
  tunnelStart,
  tunnelStatus,
  tunnelStop,
} from "../lib/bridge";
import type {
  ActivityStats,
  AuditEntry,
  CommandRun,
  FileTouch,
  McpStatus,
  ToolSurface,
  ToolUsage,
  TraceRow,
  TraceStep,
  TunnelDetection,
  TunnelStatus,
} from "../lib/types";
import { cn } from "../lib/utils";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "../components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../components/ui/dialog";
import { Input } from "../components/ui/input";
import { Progress, ProgressLabel } from "../components/ui/progress";
import { ScrollArea } from "../components/ui/scroll-area";
import { Skeleton } from "../components/ui/skeleton";
import { Stat } from "../components/ui/stat";
import { Switch } from "../components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "../components/ui/table";
import { Textarea } from "../components/ui/textarea";
import { toast } from "../components/ui/toast";
import { useTauriEvent } from "../hooks/useTauriEvent";
import { ViewShell } from "./ViewShell";

function fmtUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}m ${s}s`;
}

function fmtTs(ts: string | null | undefined): string {
  if (!ts) return "—";
  // SQLite datetime('now') is "YYYY-MM-DD HH:MM:SS"; show the time part.
  return ts.includes(" ") ? ts.split(" ")[1] : ts;
}

interface SectionProps {
  title: ReactNode;
  description?: string;
  badge?: ReactNode;
  defaultOpen?: boolean;
  children: ReactNode;
}

/**
 * Progressive disclosure: the dense secondary cards collapse behind a clear
 * header, keeping the primary controls (connector, tunnel) visible without
 * burying the reference data.
 */
function Section({
  title,
  description,
  badge,
  defaultOpen = false,
  children,
}: SectionProps) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <Card>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="group flex w-full items-start gap-2 rounded-t-xl px-(--card-spacing) text-left transition-colors hover:bg-muted/40"
      >
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="flex items-center gap-2 font-heading text-base leading-snug font-medium">
            {title}
            {badge}
          </span>
          {description && (
            <span className="text-sm text-muted-foreground">{description}</span>
          )}
        </span>
        <ChevronDownIcon
          className={cn(
            "size-4 shrink-0 text-muted-foreground transition-transform duration-200",
            open && "rotate-180",
          )}
        />
      </button>
      {open && <CardContent>{children}</CardContent>}
    </Card>
  );
}

function EmptyHint({
  icon: Icon,
  title,
  desc,
}: {
  icon: typeof ActivityIcon;
  title: string;
  desc: string;
}) {
  return (
    <div className="flex flex-col items-center gap-1.5 rounded-lg border border-dashed border-border/70 px-4 py-6 text-center">
      <Icon className="size-5 text-muted-foreground/50" />
      <p className="text-xs font-medium">{title}</p>
      <p className="max-w-56 text-[11px] leading-relaxed text-muted-foreground">
        {desc}
      </p>
    </div>
  );
}

/** The control surface: connector lifecycle, tunnel, tool surface, activity. */
export default function DashboardView({
  onOpenProject,
}: {
  onOpenProject: () => void;
}) {
  const [mcp, setMcp] = useState<McpStatus | null>(null);
  const [tunnel, setTunnel] = useState<TunnelStatus | null>(null);
  const [detections, setDetections] = useState<TunnelDetection[]>([]);
  const [surface, setSurface] = useState<ToolSurface | null>(null);
  const [stats, setStats] = useState<ActivityStats | null>(null);
  const [tools, setTools] = useState<ToolUsage[]>([]);
  const [files, setFiles] = useState<FileTouch[]>([]);
  const [commands, setCommands] = useState<CommandRun[]>([]);
  const [recent, setRecent] = useState<TraceRow[]>([]);
  const [audit, setAudit] = useState<AuditEntry[]>([]);

  const [portInput, setPortInput] = useState("");
  const [hostsInput, setHostsInput] = useState("");
  const [customCommand, setCustomCommand] = useState("");
  const [consent, setConsent] = useState<
    { provider: string; command?: string } | null
  >(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // Debounce the live trace stream: a burst of tool steps is one refresh, not
  // one IPC round-trip per event.
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refreshActivity = useCallback(async () => {
    const [s, tu, f, c, r, au, t] = await Promise.all([
      activityStats().catch(() => null),
      activityToolUsage(20).catch(() => []),
      activityFiles(50).catch(() => []),
      activityCommands(50).catch(() => []),
      activityTrace(200).catch(() => []),
      bridgeAudit(50).catch(() => []),
      tunnelStatus().catch(() => null),
    ]);
    setStats(s);
    setTools(tu);
    setFiles(f);
    setCommands(c);
    setRecent(r);
    setAudit(au);
    if (t) setTunnel(t);
  }, []);

  async function refreshAll() {
    setRefreshing(true);
    try {
      await Promise.all([
        refreshActivity(),
        activityToolSurface().then(setSurface).catch(() => null),
        tunnelDetect().then(setDetections).catch(() => []),
        mcpStatus().then(setMcp).catch(() => null),
      ]);
    } finally {
      setRefreshing(false);
    }
  }

  // Initial load: everything, plus the editable inputs seeded from live state.
  useEffect(() => {
    void (async () => {
      const [m, t, d, s] = await Promise.all([
        mcpStatus().catch(() => null),
        tunnelStatus().catch(() => null),
        tunnelDetect().catch(() => []),
        activityToolSurface().catch(() => null),
      ]);
      setMcp(m);
      setTunnel(t);
      setDetections(d);
      setSurface(s);
      if (m) {
        setPortInput(String(m.port));
        setHostsInput(m.configured_hosts.join("\n"));
      }
      await refreshActivity();
    })();
  }, [refreshActivity]);

  // Live updates from the backend, with a modest poll as the fallback.
  useTauriEvent<McpStatus>("mcp://status", (payload) => setMcp(payload));
  useTauriEvent<null>("tunnel://update", () => {
    void tunnelStatus().then(setTunnel).catch(() => null);
    void tunnelDetect().then(setDetections).catch(() => []);
  });
  useTauriEvent<TraceStep>("trace://step", () => {
    if (refreshTimer.current) clearTimeout(refreshTimer.current);
    refreshTimer.current = setTimeout(() => void refreshActivity(), 250);
  });

  useEffect(() => {
    const id = setInterval(() => void refreshActivity(), 15_000);
    return () => {
      clearInterval(id);
      if (refreshTimer.current) clearTimeout(refreshTimer.current);
    };
  }, [refreshActivity]);

  function fail(title: string, e: unknown) {
    toast.add({ title, description: String(e), type: "error" });
  }

  function copy(text: string, what: string) {
    void navigator.clipboard
      .writeText(text)
      .then(() => {
        toast.add({ title: "Copied", description: what, type: "success" });
      })
      .catch(() => {});
  }

  async function startConnector() {
    setBusy("connector");
    try {
      setMcp(await mcpStart());
    } catch (e) {
      fail("Start failed", e);
    } finally {
      setBusy(null);
    }
  }

  async function stopConnector() {
    setBusy("connector");
    try {
      setMcp(await mcpStop());
      toast.add({
        title: "Connector stopped",
        description: "connected clients are severed; in-flight calls cancelled",
        type: "info",
      });
    } catch (e) {
      fail("Stop failed", e);
    } finally {
      setBusy(null);
    }
  }

  async function restartConnector() {
    setBusy("connector");
    try {
      setMcp(await mcpRestart());
      toast.add({
        title: "Connector restarted",
        description: "connected sessions were dropped to apply the change",
        type: "info",
      });
    } catch (e) {
      fail("Restart failed", e);
    } finally {
      setBusy(null);
    }
  }

  async function applyPort() {
    const port = Number(portInput);
    if (!Number.isInteger(port) || port < 0 || port > 65535) {
      toast.add({
        title: "Invalid port",
        description: "use a number from 0 to 65535",
        type: "error",
      });
      return;
    }
    setBusy("port");
    try {
      setMcp(await mcpSetPort(port));
      toast.add({ title: "Port updated", description: String(port), type: "success" });
    } catch (e) {
      fail("Port change failed", e);
    } finally {
      setBusy(null);
    }
  }

  async function applyHosts() {
    const hosts = hostsInput
      .split(/[\n,]/)
      .map((h) => h.trim())
      .filter(Boolean);
    setBusy("hosts");
    try {
      setMcp(await mcpSetAllowedHosts(hosts));
      toast.add({
        title: "Allowlist updated",
        description: "the connector restarted to apply it",
        type: "success",
      });
    } catch (e) {
      fail("Allowlist change failed", e);
    } finally {
      setBusy(null);
    }
  }

  function requestTunnel(provider: string, command?: string) {
    setConsent({ provider, command });
  }

  async function confirmTunnel() {
    if (!consent) return;
    const { provider, command } = consent;
    setConsent(null);
    setBusy("tunnel");
    try {
      const t = await tunnelStart(provider, command);
      setTunnel(t);
      toast.add({
        title: "Tunnel starting",
        description:
          "the public host is allowlisted and the connector restarts when it appears",
        type: "info",
      });
    } catch (e) {
      fail("Tunnel start failed", e);
    } finally {
      setBusy(null);
    }
  }

  async function stopTunnel() {
    setBusy("tunnel");
    try {
      setTunnel(await tunnelStop());
      toast.add({
        title: "Tunnel stopped",
        description: "the host is withdrawn from the allowlist",
        type: "success",
      });
    } catch (e) {
      fail("Tunnel stop failed", e);
    } finally {
      setBusy(null);
    }
  }

  const stateBadge = mcp?.listening ? (
    <Badge variant="default" className="border-success/30 bg-success/10 text-success">
      Listening
    </Badge>
  ) : mcp?.running ? (
    <Badge variant="outline" className="text-warning">
      Not bound
    </Badge>
  ) : (
    <Badge variant="outline">Offline</Badge>
  );

  const maxGroup = (surface?.groups ?? []).reduce(
    (n, g) => Math.max(n, g.count),
    0,
  ) || 1;
  const maxApproval = (surface?.approvals ?? []).reduce(
    (n, a) => Math.max(n, a.count),
    0,
  ) || 1;
  const maxKind = (stats?.by_kind ?? []).reduce(
    (n, k) => Math.max(n, k.count),
    0,
  ) || 1;
  const readOnlyPct = surface
    ? Math.round((surface.read_only / surface.total) * 100)
    : 0;

  // Form friction: Apply is only useful when the field actually differs from
  // the live value — a disabled Apply is a clearer "already saved" than a
  // toast after a no-op.
  const portDirty = portInput !== String(mcp?.port ?? "");
  const hostsDirty =
    hostsInput
      .split(/[\n,]/)
      .map((h) => h.trim())
      .filter(Boolean)
      .join("\n") !== (mcp?.configured_hosts ?? []).join("\n");

  return (
    <ViewShell
      icon={ActivityIcon}
      title="Dashboard"
      description="connector lifecycle · public tunnel · activity at a glance"
      className="rounded-none border-0"
      actions={
        <Button
          size="sm"
          variant="ghost"
          disabled={refreshing}
          onClick={() => void refreshAll()}
          title="Refresh activity"
        >
          <RefreshCwIcon
            className={cn("size-3.5", refreshing && "animate-spin")}
          />
          {refreshing ? "Refreshing…" : "Refresh"}
        </Button>
      }
    >
      <div className="flex flex-col gap-3">
        {/* Trust signals — privacy/security posture up front, never buried. */}
        <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
          <span className="inline-flex items-center gap-1.5 rounded-full border border-success/30 bg-success/10 px-2.5 py-1 text-success">
            <span className="size-1.5 rounded-full bg-success" />
            Loopback only
          </span>
          <span className="inline-flex items-center gap-1.5 rounded-full border border-border/60 bg-surface-2/50 px-2.5 py-1 text-muted-foreground">
            <ShieldAlertIcon className="size-3" />
            Bearer token required
          </span>
          <span
            className={cn(
              "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1",
              mcp?.allow_write
                ? "border-warning/30 bg-warning/10 text-warning"
                : "border-success/30 bg-success/10 text-success",
            )}
          >
            <span
              className={cn(
                "size-1.5 rounded-full",
                mcp?.allow_write ? "bg-warning" : "bg-success",
              )}
            />
            {mcp?.allow_write ? "Read/write enabled" : "Read-only first"}
          </span>
          <span className="ml-auto hidden truncate font-mono text-muted-foreground sm:inline">
            {mcp?.workspace ? mcp.workspace.split(/[\\/]/).pop() : "no project bound"}
          </span>
        </div>

        {/* KPI strip */}
        {surface === null || stats === null ? (
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 xl:grid-cols-6">
            {Array.from({ length: 6 }).map((_, i) => (
              <Skeleton key={i} className="h-20 w-full" />
            ))}
          </div>
        ) : (
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 xl:grid-cols-6">
            <Stat
              label="Tools exposed"
              value={surface.total}
              sub={`${surface.read_only} read-only · ${surface.write} write`}
              icon={BoxesIcon}
            />
            <Stat label="Tool calls" value={stats.tool_calls} icon={ActivityIcon} />
            <Stat label="Files read" value={stats.files_read} icon={BookOpenIcon} />
            <Stat
              label="Files written"
              value={stats.files_written}
              icon={SquarePenIcon}
            />
            <Stat
              label="Commands run"
              value={stats.commands_run}
              icon={SquareTerminalIcon}
            />
            <Stat
              label="Failures"
              value={stats.failures}
              sub={`${stats.denied} denied`}
              icon={CircleAlertIcon}
            />
          </div>
        )}

        {/* Connector + tunnel */}
        <div className="grid gap-3 xl:grid-cols-2">
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                MCP connector
                {stateBadge}
              </CardTitle>
              <CardDescription>
                The loopback endpoint a hosted AI reaches through a tunnel.
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              {/* The one control that matters most: a single power switch. */}
              <div
                className={cn(
                  "flex items-center justify-between rounded-xl border px-3 py-3 transition-colors duration-200",
                  mcp?.listening
                    ? "border-success/30 bg-success/5"
                    : "border-border/60 bg-background/60",
                )}
              >
                <div className="flex min-w-0 items-center gap-3">
                  <span
                    className={cn(
                      "relative flex size-3 shrink-0 items-center justify-center rounded-full",
                      mcp?.listening ? "bg-success/20" : "bg-muted-foreground/10",
                    )}
                  >
                    <span
                      className={cn(
                        "size-1.5 rounded-full",
                        mcp?.listening ? "bg-success anim-pulse" : "bg-muted-foreground/50",
                      )}
                    />
                  </span>
                  <div className="flex min-w-0 flex-col">
                    <span className="text-sm font-semibold">
                      {busy === "connector"
                        ? mcp?.running
                          ? "Stopping…"
                          : "Starting…"
                        : mcp?.running
                          ? "Running"
                          : "Stopped"}
                    </span>
                    <span className="truncate text-[11px] text-muted-foreground">
                      {busy === "connector"
                        ? "please wait"
                        : mcp?.running
                          ? mcp.listening
                            ? `listening on port ${mcp.port}`
                            : "started, not bound"
                          : "switch it on to start the local MCP server"}
                    </span>
                  </div>
                </div>
                {busy === "connector" && (
                  <Loader2Icon className="size-4 shrink-0 animate-spin text-muted-foreground" />
                )}
                <Switch
                  checked={mcp?.running ?? false}
                  onCheckedChange={(on) => {
                    if (on) void startConnector();
                    else void stopConnector();
                  }}
                  disabled={busy === "connector"}
                  aria-label="MCP connector power"
                />
              </div>

              {mcp?.bind_error && (
                <div className="flex items-start gap-2 rounded-md border border-danger/30 bg-danger/10 px-2.5 py-2 text-xs text-danger">
                  <CircleAlertIcon className="mt-0.5 size-3.5 shrink-0" />
                  <span className="min-w-0 break-words">{mcp.bind_error}</span>
                </div>
              )}

              <div className="flex items-center justify-between gap-2 rounded-md border border-border/60 bg-background/60 px-3 py-2">
                <span className="shrink-0 text-[11px] text-muted-foreground">Endpoint</span>
                <div className="flex min-w-0 items-center gap-2">
                  <code
                    className="truncate font-mono text-xs"
                    title={mcp?.endpoint}
                  >
                    {mcp?.endpoint ?? "—"}
                  </code>
                  {mcp?.endpoint && (
                    <Button
                      size="icon-xs"
                      variant="ghost"
                      title="Copy endpoint"
                      onClick={() => copy(mcp.endpoint, "endpoint copied")}
                    >
                      <CopyIcon className="size-3.5" />
                    </Button>
                  )}
                </div>
              </div>

              <div className="flex items-end gap-2">
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="text-[11px] text-muted-foreground">Port</span>
                  <Input
                    value={portInput}
                    onChange={(e) => setPortInput(e.currentTarget.value)}
                    className="font-mono text-xs"
                    inputMode="numeric"
                  />
                </div>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy === "port" || !portDirty}
                  onClick={() => void applyPort()}
                >
                  Apply
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy === "connector" || !mcp?.running}
                  title="Restart to apply a port or host change"
                  onClick={() => void restartConnector()}
                >
                  <RefreshCwIcon className="size-3.5" /> Restart
                </Button>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <div className="flex flex-col gap-0.5 rounded-md border border-border/60 bg-background/60 px-3 py-2">
                  <span className="text-[11px] text-muted-foreground">Uptime</span>
                  <span className="font-mono text-xs">
                    {mcp?.running ? fmtUptime(mcp.uptime_secs) : "—"}
                  </span>
                </div>
                <div className="flex min-w-0 flex-col gap-1 rounded-md border border-border/60 bg-background/60 px-3 py-2">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-[11px] text-muted-foreground">Workspace</span>
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={onOpenProject}
                      title={mcp?.workspace ? "Change project folder" : "Select a project folder"}
                    >
                      <FolderOpenIcon className="size-3.5" />
                      {mcp?.workspace ? "Change" : "Select"}
                    </Button>
                  </div>
                  <span
                    className="truncate font-mono text-xs"
                    title={mcp?.workspace ?? ""}
                  >
                    {mcp?.workspace ?? "no project bound"}
                  </span>
                </div>
              </div>

              <div className="flex items-center justify-between gap-3 rounded-md border border-border/60 bg-background/60 px-3 py-2.5">
                <div className="flex min-w-0 flex-col">
                  <span className="text-xs font-medium">Allow writes &amp; commands</span>
                  <span className="text-[11px] leading-relaxed text-muted-foreground">
                    Off = read-only surface. Every write still needs approval.
                  </span>
                </div>
                <Switch
                  size="sm"
                  checked={mcp?.allow_write ?? false}
                  onCheckedChange={(checked) => {
                    void mcpSetAllowWrite(checked).then(setMcp);
                  }}
                />
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                Public tunnel
                {tunnel?.running && (
                  <Badge
                    variant="destructive"
                    className="border-danger/30 bg-danger/10 text-danger"
                  >
                    <ShieldAlertIcon className="size-3" /> publicly reachable
                  </Badge>
                )}
              </CardTitle>
              <CardDescription>
                Makes the local endpoint reachable from the internet. Never
                automatic — it requires explicit consent.
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              <div className="flex flex-wrap gap-1.5">
                {detections.map((d) => (
                  <Button
                    key={d.provider}
                    size="sm"
                    variant="outline"
                    disabled={busy === "tunnel" || tunnel?.running || !d.available}
                    title={
                      d.available
                        ? `${d.hint} — ${d.preview}`
                        : `${d.hint} Install it to enable this provider.`
                    }
                    onClick={() =>
                      d.provider === "custom"
                        ? requestTunnel("custom", customCommand)
                        : requestTunnel(d.provider)
                    }
                  >
                    <GlobeIcon className="size-3.5" />
                    {d.available ? d.label : `${d.label} (not installed)`}
                  </Button>
                ))}
              </div>

              <div className="flex flex-col gap-1">
                <span className="text-[11px] text-muted-foreground">
                  Custom command (<code className="font-mono">{"{port}"}</code> is
                  replaced)
                </span>
                <Input
                  value={customCommand}
                  onChange={(e) => setCustomCommand(e.currentTarget.value)}
                  placeholder="cloudflared tunnel --url http://127.0.0.1:{port}"
                  className="font-mono text-xs"
                />
              </div>

              {tunnel?.url && (
                <div className="flex items-center justify-between gap-2 rounded-md border border-border/60 bg-background/60 px-2.5 py-2">
                  <code
                    className="min-w-0 flex-1 truncate font-mono text-xs"
                    title={tunnel.url}
                  >
                    {tunnel.url}
                  </code>
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    onClick={() =>
                      tunnel.url && copy(tunnel.url, "public URL is on your clipboard")
                    }
                  >
                    <CopyIcon className="size-3.5" />
                  </Button>
                </div>
              )}

              {tunnel?.running && (
                <>
                  <Button
                    size="sm"
                    variant="destructive"
                    disabled={busy === "tunnel"}
                    onClick={() => void stopTunnel()}
                  >
                    <SquareIcon className="size-3.5" /> Stop tunnel
                  </Button>
                  {tunnel.log.length > 0 && (
                    <ScrollArea className="h-24 min-h-0 rounded-md border border-border/60 bg-background/60 p-2">
                      <pre className="whitespace-pre-wrap break-words font-mono text-[11px] leading-relaxed text-muted-foreground">
                        {tunnel.log.join("\n")}
                      </pre>
                    </ScrollArea>
                  )}
                </>
              )}

              <div className="flex flex-col gap-1">
                <span className="text-[11px] text-muted-foreground">
                  Allowed hosts (one per line — loopback is always included)
                </span>
                <Textarea
                  value={hostsInput}
                  onChange={(e) => setHostsInput(e.currentTarget.value)}
                  className="min-h-16 font-mono text-xs"
                  placeholder="tunnel host appears here automatically"
                />
                <div className="flex justify-end">
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy === "hosts" || !hostsDirty}
                    onClick={() => void applyHosts()}
                  >
                    Apply hosts
                  </Button>
                </div>
              </div>
            </CardContent>
          </Card>
        </div>

        {/* Tool surface */}
        <Section
          title="Tool surface"
          description="Derived from the engine's SPECS — the catalogue cannot drift from what the connector actually exposes."
          badge={
            <Badge variant="outline" className="font-mono text-[10px]">
              {surface?.total ?? 0} tools
            </Badge>
          }
        >
          <div className="grid gap-4 md:grid-cols-2">
            <div className="flex flex-col gap-1.5 md:col-span-2">
              <div className="flex items-center justify-between text-xs">
                <span className="text-muted-foreground">
                  Read-only vs write surface
                </span>
                <span className="font-mono text-muted-foreground">
                  {surface?.read_only ?? 0} read-only · {surface?.write ?? 0} write
                </span>
              </div>
              <div className="flex h-2 w-full overflow-hidden rounded-full bg-muted">
                <div
                  className="h-full bg-success"
                  style={{ width: `${readOnlyPct}%` }}
                />
                <div
                  className="h-full bg-warning"
                  style={{ width: `${100 - readOnlyPct}%` }}
                />
              </div>
            </div>
            <div className="flex flex-col gap-2">
              <span className="text-[11px] font-medium text-muted-foreground">
                Groups
              </span>
              {(surface?.groups ?? []).map((g) => (
                <div key={g.name} className="flex flex-col gap-0.5">
                  <Progress value={Math.round((g.count / maxGroup) * 100)}>
                    <ProgressLabel className="text-xs">{g.name}</ProgressLabel>
                    <span className="ml-auto text-xs text-muted-foreground tabular-nums">
                      {g.count}
                    </span>
                  </Progress>
                </div>
              ))}
            </div>
            <div className="flex flex-col gap-2">
              <span className="text-[11px] font-medium text-muted-foreground">
                Approval levels
              </span>
              {(surface?.approvals ?? []).map((a) => (
                <div key={a.name} className="flex flex-col gap-0.5">
                  <Progress value={Math.round((a.count / maxApproval) * 100)}>
                    <ProgressLabel className="text-xs">{a.name}</ProgressLabel>
                    <span className="ml-auto text-xs text-muted-foreground tabular-nums">
                      {a.count}
                    </span>
                  </Progress>
                </div>
              ))}
              <div className="mt-2 flex flex-col gap-1">
                <span className="text-[11px] font-medium text-muted-foreground">
                  Top tools by calls
                </span>
                {tools.length === 0 ? (
                  <p className="text-xs text-muted-foreground">
                    No tool calls recorded yet.
                  </p>
                ) : (
                  tools.slice(0, 8).map((t) => (
                    <div
                      key={t.tool}
                      className="flex items-center gap-2 text-xs"
                    >
                      <code className="min-w-0 flex-1 truncate font-mono">
                        {t.tool}
                      </code>
                      <span className="shrink-0 font-mono text-muted-foreground">
                        {t.calls}
                        {t.failures > 0 && (
                          <span className="text-danger"> · {t.failures} failed</span>
                        )}
                      </span>
                    </div>
                  ))
                )}
              </div>
            </div>
          </div>
        </Section>

        {/* Activity by kind */}
        <Section
          title="Activity by kind"
          description="What the agent has been doing, grouped by trace kind"
          badge={
            <Badge variant="outline" className="font-mono text-[10px]">
              {stats?.total ?? 0}
            </Badge>
          }
        >
          {(stats?.by_kind ?? []).length === 0 ? (
            <EmptyHint
              icon={ActivityIcon}
              title="No activity yet"
              desc="Reads, edits, commands and errors will break down here."
            />
          ) : (
            <div className="flex flex-col gap-2">
              {(stats?.by_kind ?? []).map((k) => (
                <div key={k.kind} className="flex flex-col gap-0.5">
                  <Progress value={Math.round((k.count / maxKind) * 100)}>
                    <ProgressLabel className="text-xs capitalize">
                      {k.kind}
                    </ProgressLabel>
                    <span className="ml-auto text-xs text-muted-foreground tabular-nums">
                      {k.count}
                    </span>
                  </Progress>
                </div>
              ))}
            </div>
          )}
        </Section>

        {/* Files + commands */}
        <div className="grid gap-3 xl:grid-cols-2">
          <Section
            title={
              <>
                <FileIcon className="size-4 text-muted-foreground" /> Files
              </>
            }
            description="reads vs writes per path"
            badge={
              <Badge variant="outline" className="font-mono text-[10px]">
                {stats?.files ?? 0}
              </Badge>
            }
          >
              {files.length === 0 ? (
                <EmptyHint
                  icon={FileIcon}
                  title="No files yet"
                  desc="Files the AI reads or writes will show up here."
                />
              ) : (
                <ScrollArea className="h-56 min-h-0">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>Path</TableHead>
                        <TableHead className="text-right">Reads</TableHead>
                        <TableHead className="text-right">Writes</TableHead>
                        <TableHead className="text-right">Last</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {files.map((f) => (
                        <TableRow key={f.file}>
                          <TableCell className="max-w-0 font-mono text-xs">
                            <span className="block truncate" title={f.file}>
                              {f.file}
                            </span>
                          </TableCell>
                          <TableCell className="text-right font-mono text-xs">
                            {f.reads}
                          </TableCell>
                          <TableCell className="text-right font-mono text-xs">
                            {f.writes}
                          </TableCell>
                          <TableCell className="text-right font-mono text-xs text-muted-foreground">
                            {fmtTs(f.last_ts)}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </ScrollArea>
              )}
          </Section>

          <Section
            title={
              <>
                <SquareTerminalIcon className="size-4 text-muted-foreground" /> Commands
              </>
            }
            description="runs and last outcome"
            badge={
              <Badge variant="outline" className="font-mono text-[10px]">
                {stats?.commands ?? 0}
              </Badge>
            }
          >
              {commands.length === 0 ? (
                <EmptyHint
                  icon={SquareTerminalIcon}
                  title="No commands yet"
                  desc="Shell commands the AI runs will appear here."
                />
              ) : (
                <ScrollArea className="h-56 min-h-0">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>Command</TableHead>
                        <TableHead className="text-right">Runs</TableHead>
                        <TableHead className="text-right">Last</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {commands.map((c) => (
                        <TableRow key={c.command}>
                          <TableCell className="max-w-0 font-mono text-xs">
                            <span className="block truncate" title={c.command}>
                              {c.command}
                            </span>
                          </TableCell>
                          <TableCell className="text-right font-mono text-xs">
                            {c.runs}
                          </TableCell>
                          <TableCell className="text-right">
                            <span
                              className={cn(
                                "inline-flex items-center gap-1 font-mono text-xs",
                                c.last_ok ? "text-success" : "text-danger",
                              )}
                            >
                              {c.last_ok ? (
                                <CheckIcon className="size-3" />
                              ) : (
                                <XIcon className="size-3" />
                              )}
                              {fmtTs(c.last_ts)}
                            </span>
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </ScrollArea>
              )}
          </Section>
        </div>

        {/* Audit trail */}
        <Section
          title="Audit trail"
          description="Approvals and denials, newest first"
          badge={
            <Badge variant="outline" className="font-mono text-[10px]">
              {audit.length}
            </Badge>
          }
        >
          {audit.length === 0 ? (
            <EmptyHint
              icon={ShieldAlertIcon}
              title="No decisions yet"
              desc="Approved and denied tool calls appear here."
            />
          ) : (
            <ScrollArea className="h-56 min-h-0">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Time</TableHead>
                    <TableHead>Agent</TableHead>
                    <TableHead>Tool</TableHead>
                    <TableHead>Decision</TableHead>
                    <TableHead className="text-right">Outcome</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {audit.slice(0, 100).map((a, i) => (
                    <TableRow key={`${a.ts}-${i}`}>
                      <TableCell className="font-mono text-xs text-muted-foreground">
                        {fmtTs(a.ts)}
                      </TableCell>
                      <TableCell>
                        <Badge variant="outline" className="font-mono text-[10px]">
                          {a.agent}
                        </Badge>
                      </TableCell>
                      <TableCell className="max-w-0 font-mono text-xs">
                        <span className="block truncate" title={a.tool}>
                          {a.tool}
                        </span>
                      </TableCell>
                      <TableCell>
                        <span
                          className={cn(
                            "inline-flex items-center gap-1 text-xs",
                            a.allowed ? "text-success" : "text-danger",
                          )}
                        >
                          {a.allowed ? (
                            <CheckIcon className="size-3" />
                          ) : (
                            <XIcon className="size-3" />
                          )}
                          {a.allowed ? `allowed (${a.approved_by})` : "denied"}
                        </span>
                      </TableCell>
                      <TableCell className="text-right">
                        <span
                          className={cn(
                            "text-xs",
                            a.ok ? "text-success" : "text-danger",
                          )}
                        >
                          {a.ok ? "ok" : "failed"}
                        </span>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </ScrollArea>
          )}
        </Section>

        {/* Recent activity */}
        <Card>
          <CardHeader>
            <CardTitle>Recent activity</CardTitle>
            <CardDescription>
              Persisted trace, newest first — survives a reload, not just the
              live stream.
            </CardDescription>
          </CardHeader>
          <CardContent>
            {recent.length === 0 ? (
              <EmptyHint
                icon={ActivityIcon}
                title="No activity yet"
                desc="Tool calls stream in here as the AI works."
              />
            ) : (
              <ScrollArea className="h-56 min-h-0">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Time</TableHead>
                      <TableHead>Source</TableHead>
                      <TableHead>Tool</TableHead>
                      <TableHead>Target</TableHead>
                      <TableHead className="text-right">Outcome</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {recent.slice(0, 100).map((r, i) => (
                      <TableRow key={`${r.ts}-${i}`}>
                        <TableCell className="font-mono text-xs text-muted-foreground">
                          {fmtTs(r.ts)}
                        </TableCell>
                        <TableCell>
                          <Badge
                            variant="outline"
                            className="font-mono text-[10px]"
                          >
                            {r.source ?? "—"}
                          </Badge>
                        </TableCell>
                        <TableCell className="max-w-0 font-mono text-xs">
                          <span className="block truncate" title={r.tool ?? ""}>
                            {r.tool ?? r.kind}
                          </span>
                        </TableCell>
                        <TableCell className="max-w-0 font-mono text-xs text-muted-foreground">
                          <span
                            className="block truncate"
                            title={r.file ?? r.command ?? r.detail ?? ""}
                          >
                            {r.file ?? r.command ?? r.detail ?? "—"}
                          </span>
                        </TableCell>
                        <TableCell className="text-right">
                          <span
                            className={cn(
                              "inline-flex items-center gap-1 text-xs",
                              r.ok ? "text-success" : "text-danger",
                            )}
                          >
                            {r.ok ? (
                              <CheckIcon className="size-3" />
                            ) : (
                              <XIcon className="size-3" />
                            )}
                            {r.ok ? "ok" : "failed"}
                          </span>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </ScrollArea>
            )}
          </CardContent>
        </Card>
      </div>

      {/* Tunnel consent: the one action that can expose the machine. */}
      <Dialog
        open={consent !== null}
        onOpenChange={(open) => {
          if (!open) setConsent(null);
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <ShieldAlertIcon className="size-4 text-danger" />
              Expose the connector publicly?
            </DialogTitle>
            <DialogDescription>
              {consent?.provider === "custom"
                ? "This runs the command you typed and publishes whatever HTTPS URL it prints."
                : `This starts ${consent?.provider} and publishes the HTTPS URL it prints.`}
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-2 text-xs leading-relaxed text-muted-foreground">
            <p>
              A tunnel makes a local tool server reachable from the internet.
              The bearer token is the only thing between that URL and your
              project's read surface.
            </p>
            <p>
              Responses are signed, but traffic is only as confidential as the
              tunnel's TLS. Stop the tunnel — or quit the app — to close it.
            </p>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConsent(null)}>
              Cancel
            </Button>
            <Button variant="destructive" onClick={() => void confirmTunnel()}>
              Start tunnel
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </ViewShell>
  );
}
