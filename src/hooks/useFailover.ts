import { useEffect, useState } from "react";
import { failoverReset, failoverStatus } from "../lib/bridge";
import type {
  FailoverLocalEvent,
  FailoverStatus,
  FailoverWebEvent,
} from "../lib/types";
import { useTauriEvent } from "./useTauriEvent";

/**
 * Owns the failover state machines' UI state in one place: the periodic
 * status snapshot, the one-shot local/web interruption events, and the
 * dismiss action. Previously both `Statusbar` and `FailoverBanner` fetched
 * and subscribed independently — two IPC calls and two listeners for the
 * same data, which could disagree.
 */
export function useFailover() {
  const [status, setStatus] = useState<FailoverStatus | null>(null);
  const [localEvent, setLocalEvent] = useState<FailoverLocalEvent | null>(null);
  const [webEvent, setWebEvent] = useState<FailoverWebEvent | null>(null);

  useEffect(() => {
    void failoverStatus()
      .then(setStatus)
      .catch(() => {});
  }, []);

  useTauriEvent<FailoverStatus>("failover://status", setStatus);
  useTauriEvent<FailoverLocalEvent>("failover://local", setLocalEvent);
  useTauriEvent<FailoverWebEvent>("failover://web", setWebEvent);

  function dismiss(agent: "local" | "web") {
    void failoverReset(agent).then(() => {
      if (agent === "local") setLocalEvent(null);
      else setWebEvent(null);
    });
  }

  return { status, localEvent, webEvent, dismiss };
}
