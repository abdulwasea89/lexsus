import { useEffect, useState } from "react";
import { bridgeApprove, bridgeGrantState } from "../lib/bridge";
import type { ApprovalRequested, GrantState } from "../lib/types";
import { toast } from "../components/ui/toast";
import { useTauriEvent } from "./useTauriEvent";

export interface Approval extends ApprovalRequested {
  resolving?: boolean;
}

/** What a grant offer looks like on the wire to `bridge_approve`. */
export interface GrantChoice {
  scope: "editing" | "commands";
  path_prefix: string | null;
}

/**
 * Owns the web-AI approval queue so exactly one component (the global
 * banner) renders it. Mirrors `bridge://approval-requested/-resolved` and
 * `bridge://grants-changed` (the session-grant state the GrantsBar shows).
 */
export function useApprovals() {
  const [approvals, setApprovals] = useState<Approval[]>([]);
  const [grantState, setGrantState] = useState<GrantState>({
    grants: [],
    paused: false,
  });

  useEffect(() => {
    void bridgeGrantState()
      .then(setGrantState)
      .catch(() => setGrantState({ grants: [], paused: false }));
  }, []);

  useTauriEvent<ApprovalRequested>("bridge://approval-requested", (payload) => {
    setApprovals((prev) => [
      { ...payload, id: payload.id },
      ...prev.filter((p) => p.id !== payload.id),
    ]);
  });

  useTauriEvent<{ id: number }>("bridge://approval-resolved", (payload) => {
    setApprovals((prev) => prev.filter((p) => p.id !== payload.id));
  });

  useTauriEvent<GrantState>("bridge://grants-changed", setGrantState);

  async function decide(id: number, allow: boolean, grant?: GrantChoice) {
    setApprovals((prev) =>
      prev.map((p) => (p.id === id ? { ...p, resolving: true } : p)),
    );
    try {
      await bridgeApprove(id, allow, grant);
      // Only clear the card once the core confirms the decision landed.
      setApprovals((prev) => prev.filter((p) => p.id !== id));
    } catch (e) {
      // A failed decision must stay actionable: re-enable it and say so,
      // rather than silently dropping a still-pending request.
      setApprovals((prev) =>
        prev.map((p) => (p.id === id ? { ...p, resolving: false } : p)),
      );
      toast.add({
        title: allow ? "Approval failed" : "Denial failed",
        description: String(e),
        type: "error",
      });
    }
  }

  return { approvals, grantState, decide };
}
