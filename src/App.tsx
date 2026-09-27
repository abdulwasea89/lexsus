import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  getProjectRoot,
  mcpStatus,
  setProjectRoot,
  startWatch,
  tunnelStatus,
} from "./lib/bridge";
import type { McpStatus, TunnelStatus } from "./lib/types";
import ApprovalBanner from "./components/ApprovalBanner";
import ErrorBoundary from "./components/ErrorBoundary";
import GrantsBar from "./components/GrantsBar";
import DashboardView from "./views/DashboardView";
import FailoverBanner from "./components/FailoverBanner";
import ProjectDialog from "./components/ProjectDialog";
import Onboarding from "./components/Onboarding";
import QuestionBanner from "./components/QuestionBanner";
import Statusbar from "./components/Statusbar";
import Titlebar from "./components/Titlebar";
import { isOnboarded } from "./lib/onboarding";
import { useApprovals } from "./hooks/useApprovals";
import { useFailover } from "./hooks/useFailover";
import { useQuestions } from "./hooks/useQuestions";
import { useTauriEvent } from "./hooks/useTauriEvent";
import { toast } from "./components/ui/toast";

const RECENTS_KEY = "lexsus.recentProjects";

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

/**
 * The whole app is the dashboard: connector lifecycle, public tunnel and
 * activity. Onboarding and the first-run project picker are untouched.
 */
export default function App() {
  const [projectRoot, setRootInput] = useState("");
  const [restored, setRestored] = useState(false);
  const [mcp, setMcp] = useState<McpStatus | null>(null);
  const [tunnel, setTunnel] = useState<TunnelStatus | null>(null);
  const [recents, setRecents] = useState<string[]>([]);
  const [projectOpen, setProjectOpen] = useState(false);
  const [showOnboarding, setShowOnboarding] = useState(() => !isOnboarded());
  const { approvals, grantState, decide } = useApprovals();
  const { questions, answer } = useQuestions();
  const { status, localEvent, webEvent, dismiss } = useFailover();
  // Guards project switches: a slow switch must not clobber a newer one.
  const switchToken = useRef(0);

  useEffect(() => {
    setRecents(loadRecents());
  }, []);

  // Dev-only: Ctrl+Shift+O replays the onboarding stage. The DEV guard is
  // compiled out of production builds, so real users never hit this.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "o") {
        e.preventDefault();
        setShowOnboarding(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    void (async () => {
      const [saved, connector, tunnelState] = await Promise.all([
        getProjectRoot().catch(() => null),
        mcpStatus().catch(() => null),
        tunnelStatus().catch(() => null),
      ]);
      setMcp(connector);
      setTunnel(tunnelState);
      if (saved) {
        setRootInput(saved);
        saveRecent(saved);
        setRecents(loadRecents());
        try {
          await startWatch();
        } catch {
          // The saved folder no longer exists (deleted or renamed). Don't
          // error — clear it and hand back to the project picker.
          setRootInput("");
          setProjectOpen(true);
        }
      } else if (isOnboarded()) {
        // First-run users meet the onboarding stage first; the project
        // dialog opens when they finish it.
        setProjectOpen(true);
      }
      setRestored(true);
    })();
  }, []);

  // Keep the statusbar in step with lifecycle changes made in the dashboard.
  useTauriEvent<McpStatus>("mcp://status", (payload) => setMcp(payload));
  useTauriEvent<null>("tunnel://update", () => {
    void tunnelStatus().then(setTunnel).catch(() => null);
  });

  async function applyProject(path: string) {
    const token = ++switchToken.current;
    try {
      await setProjectRoot(path);
      await startWatch();
      // The connector's blast radius follows the bound workspace.
      const connector = await mcpStatus().catch(() => null);
      if (token !== switchToken.current) return;
      setMcp(connector);
      saveRecent(path);
      setRecents(loadRecents());
    } catch (e) {
      if (token !== switchToken.current) return;
      toast.add({
        title: "Could not open project",
        description: String(e),
        type: "error",
      });
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
      toast.add({
        title: "Could not browse",
        description: String(e),
        type: "error",
      });
    }
  }

  function onPickProject(path: string) {
    setRootInput(path);
    void applyProject(path);
  }

  function finishOnboarding() {
    setShowOnboarding(false);
    // Hand off to the project picker if nothing is bound yet.
    if (!projectRoot) setProjectOpen(true);
  }

  return (
    <div className="flex h-screen w-full flex-col overflow-hidden bg-background text-foreground">
      <Titlebar />

      <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <ApprovalBanner approvals={approvals} onDecide={decide} />
        <QuestionBanner questions={questions} onAnswer={answer} />
        <GrantsBar grantState={grantState} />
        <FailoverBanner
          status={status}
          localEvent={localEvent}
          webEvent={webEvent}
          dismiss={dismiss}
        />

        {!restored ? (
          <div className="flex flex-1 items-center justify-center p-8 text-sm text-muted-foreground">
            <span className="animate-pulse">restoring session…</span>
          </div>
        ) : (
          <ErrorBoundary label="Dashboard">
            <div className="flex min-h-0 flex-1 flex-col p-3">
              <div className="min-h-0 flex-1">
                <DashboardView />
              </div>
            </div>
          </ErrorBoundary>
        )}

        <Statusbar
          projectRoot={projectRoot}
          connector={mcp}
          tunnel={tunnel}
          status={status}
        />
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

      {showOnboarding && <Onboarding onDone={finishOnboarding} />}
    </div>
  );
}
