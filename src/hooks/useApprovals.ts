import { useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { bridgeApprove, bridgeGrantState } from "../lib/bridge";
import type { ApprovalRequested, GrantState } from "../lib/types";

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
  const mounted = useRef(false);

  useEffect(() => {
    mounted.current = true;
    let unlistens: UnlistenFn[] = [];
    void (async () => {
      setGrantState(await bridgeGrantState().catch(() => ({ grants: [], paused: false })));
      unlistens = [
        await listen<ApprovalRequested>("bridge://approval-requested", (e) => {
          if (!mounted.current) return;
          setApprovals((prev) => [
            { ...e.payload, id: e.payload.id },
            ...prev.filter((p) => p.id !== e.payload.id),
          ]);
        }),
        await listen<{ id: number }>("bridge://approval-resolved", (e) => {
          if (!mounted.current) return;
          setApprovals((prev) => prev.filter((p) => p.id !== e.payload.id));
        }),
        await listen<GrantState>("bridge://grants-changed", (e) => {
          if (!mounted.current) return;
          setGrantState(e.payload);
        }),
      ];
    })();
    return () => {
      mounted.current = false;
      for (const u of unlistens) u();
    };
  }, []);

  async function decide(id: number, allow: boolean, grant?: GrantChoice) {
    setApprovals((prev) =>
      prev.map((p) => (p.id === id ? { ...p, resolving: true } : p)),
    );
    await bridgeApprove(id, allow, grant).catch(() => {});
    setApprovals((prev) => prev.filter((p) => p.id !== id));
  }

  return { approvals, grantState, decide };
}
