import { useState } from "react";
import { ShieldAlertIcon, Trash2Icon } from "lucide-react";
import type { Approval } from "../hooks/useApprovals";
import { Button } from "./ui/button";

interface ApprovalBannerProps {
  approvals: Approval[];
  onDecide: (
    id: number,
    allow: boolean,
    grant?: { scope: "editing" | "commands"; path_prefix: string | null },
  ) => void;
}

/** The label for a grant offer: "edits under src/" or "commands". */
function grantLabel(scope: string, prefix: string | null): string {
  if (scope === "commands") return "commands";
  return prefix ? `edits under ${prefix}/` : "edits in this project";
}

/**
 * Global approval gate: when a web AI wants to write a file or run a
 * command, this banner sits above everything until you allow or deny it.
 * Destructive calls (delete, move) render as danger and show exactly what
 * disappears; grantable calls can be promoted to a session grant.
 */
export default function ApprovalBanner({
  approvals,
  onDecide,
}: ApprovalBannerProps) {
  // Which cards have their "don't ask again" box ticked, by approval id.
  const [grantWanted, setGrantWanted] = useState<Record<number, boolean>>({});

  if (approvals.length === 0) return null;

  return (
    <div className="flex shrink-0 flex-col gap-1.5 border-b border-warning/30 bg-warning/10 px-4 py-2.5">
      {approvals.map((a) => {
        const grant = a.grantable;
        const wantsGrant = grantWanted[a.id] ?? false;
        return (
          <div
            key={a.id}
            className="flex flex-wrap items-center gap-2.5"
            role="alert"
          >
            {a.destructive ? (
              <Trash2Icon className="size-4 shrink-0 text-danger" />
            ) : (
              <ShieldAlertIcon className="size-4 shrink-0 text-warning" />
            )}
            <p className="min-w-0 flex-1 text-sm">
              <span
                className={`font-semibold ${a.destructive ? "text-danger" : "text-warning"}`}
              >
                {a.source === "web" ? "Web AI" : "Desktop"} requests:
              </span>{" "}
              <span className="font-mono text-xs">
                {a.summary}
              </span>
              {a.destructive && (
                <span className="ml-1.5 text-danger">
                  — this may destroy work
                </span>
              )}
            </p>
            {grant && (
              <label className="flex cursor-pointer select-none items-center gap-1.5 text-xs text-muted-foreground">
                <input
                  type="checkbox"
                  className="size-3.5 accent-primary"
                  checked={wantsGrant}
                  onChange={(e) =>
                    setGrantWanted((prev) => ({
                      ...prev,
                      [a.id]: e.target.checked,
                    }))
                  }
                />
                Don't ask again for{" "}
                {grantLabel(grant.scope, grant.suggested_prefix)} this session
              </label>
            )}
            <div className="flex gap-1.5">
              <Button
                size="sm"
                variant={a.destructive ? "destructive" : "default"}
                disabled={a.resolving}
                onClick={() =>
                  onDecide(
                    a.id,
                    true,
                    grant && wantsGrant
                      ? {
                          scope: grant.scope,
                          path_prefix: grant.suggested_prefix,
                        }
                      : undefined,
                  )
                }
              >
                Allow
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={a.resolving}
                onClick={() => onDecide(a.id, false)}
              >
                Deny
              </Button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
