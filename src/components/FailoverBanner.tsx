import {
  AlertTriangleIcon,
  GlobeIcon,
  ShieldAlertIcon,
  SparklesIcon,
} from "lucide-react";
import type {
  FailoverLocalEvent,
  FailoverStatus,
  FailoverWebEvent,
} from "../lib/types";
import { Button } from "./ui/button";

function fmtIdle(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  const m = Math.floor(s / 60);
  const sec = s % 60;
  if (m === 0) return `${sec}s`;
  return `${m}m ${sec.toString().padStart(2, "0")}s`;
}

interface FailoverBannerProps {
  status: FailoverStatus | null;
  localEvent: FailoverLocalEvent | null;
  webEvent: FailoverWebEvent | null;
  dismiss: (agent: "local" | "web") => void;
}

/**
 * Failover alerts promoted from the old panel into the global banner
 * stack: a stalled local agent or an interrupted session. Renders nothing
 * when all is quiet — the quiet local/web states live in the statusbar
 * instead.
 *
 * Delivery is in-app by design: the MCP connector is pull-based, so the
 * app surfaces the interruption and hands the user the built handoff
 * rather than pushing into a chat.
 */
export default function FailoverBanner({
  status,
  localEvent,
  webEvent,
  dismiss,
}: FailoverBannerProps) {
  const local = status?.local ?? "inactive";

  const localStalled = local === "stalled" && !localEvent;
  const localError = localEvent && !localEvent.ok;
  const localDelivered = !!localEvent?.ok;

  if (!localStalled && !localError && !localDelivered && !webEvent) {
    return null;
  }

  return (
    <div className="flex shrink-0 flex-col gap-0.5 border-b border-border/60 bg-surface-2/60 px-4 py-2 text-xs anim-fade-down">
      {localStalled && (
        <div className="flex items-center gap-2">
          <AlertTriangleIcon className="size-4 shrink-0 text-warning" />
          <p className="min-w-0 flex-1 text-muted-foreground">
            Your local terminal has been idle — if you stopped working, the
            bridge can continue automatically.
          </p>
          <Button variant="ghost" size="sm" onClick={() => dismiss("local")}>
            Keep working
          </Button>
        </div>
      )}

      {localError && (
        <div className="flex items-center gap-2">
          <ShieldAlertIcon className="size-4 shrink-0 text-danger" />
          <p className="min-w-0 flex-1 font-medium text-danger">
            Could not build a handoff for the interrupted local session
            {localEvent.error && (
              <span className="ml-2 font-normal text-muted-foreground">
                {localEvent.error}
              </span>
            )}
          </p>
          <Button variant="ghost" size="sm" onClick={() => dismiss("local")}>
            Dismiss
          </Button>
        </div>
      )}

      {localDelivered && (
        <div className="flex items-center gap-2">
          {localEvent.delivered ? (
            <SparklesIcon className="size-4 shrink-0 text-success" />
          ) : (
            <ShieldAlertIcon className="size-4 shrink-0 text-warning" />
          )}
          <p className="min-w-0 flex-1 font-medium">
            {localEvent.delivered
              ? "Local session interrupted — auto-continued on the web AI"
              : "Local session interrupted — continue from the Handoff view"}
            {localEvent.idle_ms != null && (
              <span className="ml-2 font-normal text-muted-foreground">
                after {fmtIdle(localEvent.idle_ms)}
              </span>
            )}
          </p>
          <Button variant="ghost" size="sm" onClick={() => dismiss("local")}>
            Dismiss
          </Button>
        </div>
      )}

      {webEvent && (
        <div className="flex flex-col gap-2 py-1">
          <div className="flex items-center gap-2">
            <GlobeIcon className="size-4 shrink-0 text-warning" />
            <p className="min-w-0 flex-1 font-medium">
              Web AI session lost (
              {webEvent.trigger === "ws_drop" ? "disconnected" : "went idle"})
              <span className="ml-2 font-normal text-muted-foreground">
                after {fmtIdle(webEvent.idle_ms)}
              </span>
            </p>
            <span className="hidden text-muted-foreground md:inline">
              pick the work back up from the Handoff view
            </span>
            <Button variant="ghost" size="sm" onClick={() => dismiss("web")}>
              Dismiss
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
