import { useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { CircleAlertIcon, SquareTerminalIcon } from "lucide-react";
import {
  getProjectRoot,
  pairGetCode,
  pairStatus,
  setProjectRoot,
  startWatch,
} from "./lib/bridge";
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
 * Workbench shell: icon rail (views + project/pairing), a persistent
 * terminal on the left, the active view on the right, global approval
 * and failover banners on top, and a statusbar heartbeat below.
 */
export default function App() {
  const [projectRoot, setRootInput] = useState("");
  const [restored, setRestored] = useState(false);
  const [error, setError] = useState("");
  const [pairCode, setPairCode] = useState("");
  const [paired, setPaired] = useState(false);
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
    let unlistens: UnlistenFn[] = [];
    void (async () => {
      try {
        const [saved, code, isPaired] = await Promise.all([
          getProjectRoot(),
          pairGetCode().catch(() => ""),
          pairStatus().catch(() => false),
        ]);
        if (saved) {
          setRootInput(saved);
          saveRecent(saved);
          setRecents(loadRecents());
          await startWatch();
        } else {
          setProjectOpen(true);
        }
        setPairCode(code);
        setPaired(isPaired);
        unlistens = [
          await listen<string>("pair://code", (e) => setPairCode(e.payload)),
          await listen<boolean>("pair://status", (e) => setPaired(e.payload)),
        ];
      } catch (e) {
        setError(String(e));
      } finally {
        setRestored(true);
      }
    })();
    return () => {
      for (const u of unlistens) u();
    };
  }, []);

  async function applyProject(path: string) {
    try {
      await setProjectRoot(path);
      await startWatch();
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
          paired={paired}
          onOpenProject={() => setProjectOpen(true)}
        />

        <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <ApprovalBanner approvals={approvals} onDecide={decide} />
        <GrantsBar grantState={grantState} />
        <FailoverBanner />

        {error && (
          <Alert variant="destructive" className="m-3 mb-0">
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
            <div className="flex h-[50vh] min-h-0 shrink-0 flex-col lg:h-auto lg:w-[55%] lg:shrink">
              {projectRoot ? (
                <TerminalPane />
              ) : (
                <section className="flex h-full min-h-0 flex-col overflow-hidden rounded-lg border border-border bg-surface">
                  <header className="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2.5">
                    <SquareTerminalIcon className="size-4 shrink-0 text-muted-foreground" />
                    <h2 className="text-sm font-semibold">Terminal</h2>
                    <span className="ml-auto text-xs text-muted-foreground">
                      locked
                    </span>
                  </header>
                  <div className="flex flex-1 flex-col items-center justify-center gap-2 p-8 text-center">
                    <SquareTerminalIcon className="size-8 text-muted-foreground/50" />
                    <p className="text-sm font-medium">Terminal locked</p>
                    <p className="max-w-64 text-xs leading-relaxed text-muted-foreground">
                      Pick a project folder to watch the web AI run commands
                      here.
                    </p>
                    <button
                      type="button"
                      onClick={() => setProjectOpen(true)}
                      className="text-xs text-primary underline-offset-4 hover:underline"
                    >
                      Choose a folder →
                    </button>
                  </div>
                </section>
              )}
            </div>

            <div className="flex min-h-0 flex-1 flex-col">
              {view === "trace" && <TraceView />}
              {view === "git" && <GitView />}
              {view === "handoff" && <HandoffView />}
              {view === "memory" && <MemoryView />}
              {view === "bridge" && <BridgeView />}
            </div>
          </div>
        )}

        <Statusbar projectRoot={projectRoot} paired={paired} />
      </main>

      <ProjectDialog
        open={projectOpen}
        onOpenChange={setProjectOpen}
        projectRoot={projectRoot}
        recents={recents}
        pairCode={pairCode}
        paired={paired}
        onPick={onPickProject}
        onBrowse={() => void onBrowse()}
      />
      </div>
    </div>
  );
}
