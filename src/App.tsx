import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { CircleAlertIcon } from "lucide-react";
import {
  getProjectRoot,
  mcpStatus,
  setProjectRoot,
  startWatch,
} from "./lib/bridge";
import type { McpStatus } from "./lib/types";
import ApprovalBanner from "./components/ApprovalBanner";
import GrantsBar from "./components/GrantsBar";
import BridgeView from "./views/BridgeView";
import FailoverBanner from "./components/FailoverBanner";
import GitView from "./views/GitView";
import HandoffView from "./views/HandoffView";
import MemoryView from "./views/MemoryView";
import ProjectDialog from "./components/ProjectDialog";
import TraceView from "./views/TraceView";
import Statusbar from "./components/Statusbar";
import TerminalPane from "./components/TerminalPane";
import Titlebar from "./components/Titlebar";
import WorkbenchRail, { type View } from "./components/WorkbenchRail";
import { useApprovals } from "./hooks/useApprovals";
import { Alert, AlertDescription, AlertTitle } from "./components/ui/alert";
import GettingStarted from "./components/GettingStarted";

const RECENTS_KEY = "lexsus.recentProjects";
const VIEW_KEY = "lexsus.view";
const VIEWS: View[] = ["trace", "git", "handoff", "memory", "bridge"];

function loadRecents(): string[] {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    if (!raw) return [];
    const arr = JSON.parse(raw);
    return Array.isArray(arr)
      ? arr.filter((p): p is string => typeof p === "string")
      : [];
  } catch {
    return [];
  }
}

function saveRecent(path: string) {
  const next = [path, ...loadRecents().filter((p) => p !== path)].slice(0, 8);
  localStorage.setItem(RECENTS_KEY, JSON.stringify(next));
}

function loadView(): View {
  const v = localStorage.getItem(VIEW_KEY) as View | null;
  return v && VIEWS.includes(v) ? v : "trace";
}

/**
 * Workbench shell: icon rail (views + project/connector), a persistent
 * terminal on the left, the active view on the right, global approval
 * and failover banners on top, and a statusbar heartbeat below.
 */
export default function App() {
  const [projectRoot, setRootInput] = useState("");
  const [restored, setRestored] = useState(false);
  const [error, setError] = useState("");
  const [mcp, setMcp] = useState<McpStatus | null>(null);
  const [recents, setRecents] = useState<string[]>([]);
  const [view, setView] = useState<View>(loadView);
  const [projectOpen, setProjectOpen] = useState(false);
  const { approvals, grantState, decide } = useApprovals();

  useEffect(() => {
    localStorage.setItem(VIEW_KEY, view);
  }, [view]);

  useEffect(() => {
    setRecents(loadRecents());
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const [saved, connector] = await Promise.all([
          getProjectRoot(),
          mcpStatus().catch(() => null),
        ]);
        if (saved) {
          setRootInput(saved);
          saveRecent(saved);
          setRecents(loadRecents());
          await startWatch();
        } else {
          setProjectOpen(true);
        }
        setMcp(connector);
      } catch (e) {
        setError(String(e));
      } finally {
        setRestored(true);
      }
    })();
  }, []);

  async function applyProject(path: string) {
    try {
      await setProjectRoot(path);
      await startWatch();
      // The connector's blast radius follows the bound workspace.
      setMcp(await mcpStatus().catch(() => null));
      setError("");
      saveRecent(path);
      setRecents(loadRecents());
    } catch (e) {
      setError(String(e));
    }
  }

  async function onBrowse() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select project folder",
      });
      if (typeof selected === "string" && selected) {
        setRootInput(selected);
        await applyProject(selected);
      }
    } catch (e) {
      setError(String(e));
    }
  }

  function onPickProject(path: string) {
    setRootInput(path);
    void applyProject(path);
  }

  return (
    <div className="flex h-screen w-full flex-col overflow-hidden bg-background text-foreground">
      <Titlebar />

      <div className="flex min-h-0 flex-1 overflow-hidden">
        <WorkbenchRail
          view={view}
          onViewChange={setView}
          connector={mcp}
          onOpenProject={() => setProjectOpen(true)}
        />

        <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <ApprovalBanner approvals={approvals} onDecide={decide} />
        <GrantsBar grantState={grantState} />
        <FailoverBanner />

        {error && (
          <Alert variant="destructive" className="m-3 mb-0 anim-pop">
            <CircleAlertIcon />
            <AlertTitle>Something went wrong</AlertTitle>
            <AlertDescription className="font-mono text-xs">
              {error}
            </AlertDescription>
          </Alert>
        )}

        {!restored ? (
          <div className="flex flex-1 items-center justify-center p-8 text-sm text-muted-foreground">
            <span className="animate-pulse">restoring session…</span>
          </div>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col gap-3 p-3 lg:flex-row">
            <div className="flex h-[50vh] min-h-0 shrink-0 flex-col lg:h-auto lg:w-[55%] lg:shrink anim-fade-up">
              {projectRoot ? (
                <TerminalPane />
              ) : (
                <GettingStarted onOpenProject={() => setProjectOpen(true)} />
              )}
            </div>

            <div className="flex min-h-0 flex-1 flex-col">
              <div key={view} className="h-full anim-fade-up">
                {view === "trace" && <TraceView />}
                {view === "git" && <GitView />}
                {view === "handoff" && <HandoffView />}
                {view === "memory" && <MemoryView />}
                {view === "bridge" && <BridgeView />}
              </div>
            </div>
          </div>
        )}

        <Statusbar projectRoot={projectRoot} connector={mcp} />
      </main>

      <ProjectDialog
        open={projectOpen}
        onOpenChange={setProjectOpen}
        projectRoot={projectRoot}
        recents={recents}
        connector={mcp}
        onPick={onPickProject}
        onBrowse={() => void onBrowse()}
      />
      </div>
    </div>
  );
}
