import { OctagonPauseIcon, XIcon } from "lucide-react";
import { bridgeGrantRevoke, bridgePause } from "../lib/bridge";
import type { GrantState } from "../lib/types";
import { Button } from "./ui/button";

interface GrantsBarProps {
  grantState: GrantState;
}

/** Human form of one grant: "edits under src/" / "commands". */
function grantText(scope: string, prefix: string | null): string {
  if (scope === "commands") return "commands";
  return prefix ? `edits under ${prefix}/` : "edits (whole project)";
}

/**
 * Session-grant indicator: every active auto-approval as a revocable chip,
 * plus the kill switch — one click revokes every grant and pauses the
 * bridge. Visible whenever anything is granted or the bridge is paused, so
 * a standing permission never sits quietly out of sight.
 */
export default function GrantsBar({ grantState }: GrantsBarProps) {
  const { grants, paused } = grantState;
  if (grants.length === 0 && !paused) return null;

  return (
    <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border bg-muted/40 px-4 py-1.5 text-xs">
      {paused ? (
        <span className="flex items-center gap-1.5 font-semibold text-danger">
          <OctagonPauseIcon className="size-3.5" />
          Bridge paused — no tool calls run
        </span>
      ) : (
        <span className="text-muted-foreground">Session grants:</span>
      )}
      {grants.map((g) => (
        <span
          key={g.id}
          className="inline-flex items-center gap-1 rounded-full bg-background px-2 py-0.5 font-mono"
          title={`auto-approves ${grantText(g.scope, g.path_prefix)} from ${g.source}`}
        >
          {grantText(g.scope, g.path_prefix)}
          <button
            className="text-muted-foreground hover:text-danger"
            aria-label={`revoke grant for ${grantText(g.scope, g.path_prefix)}`}
            onClick={() => void bridgeGrantRevoke(g.id).catch(() => {})}
          >
            <XIcon className="size-3" />
          </button>
        </span>
      ))}
      <span className="flex-1" />
      {paused ? (
        <Button size="sm" variant="outline" onClick={() => void bridgePause(false).catch(() => {})}>
          Resume bridge
        </Button>
      ) : (
        <Button
          size="sm"
          variant="outline"
          className="text-danger"
          onClick={() => void bridgePause(true).catch(() => {})}
        >
          Revoke all &amp; pause
        </Button>
      )}
    </div>
  );
}
