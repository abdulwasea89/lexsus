import { useEffect, useRef } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Subscribe to one Tauri event with a lifecycle-safe registration.
 *
 * `listen` resolves asynchronously, so a component that unmounts before the
 * promise settles would otherwise run its cleanup with an empty unlisten
 * handle — leaking a listener that keeps firing `setState` on every event.
 * This registers, and if the effect is disposed before/while resolving,
 * immediately unlistens. The handler is kept in a ref so a fresh closure
 * each render never forces a re-subscribe.
 */
export function useTauriEvent<T>(event: string, onEvent: (payload: T) => void) {
  const handlerRef = useRef(onEvent);
  handlerRef.current = onEvent;

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void listen<T>(event, (e) => {
      if (!disposed) handlerRef.current(e.payload);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [event]);
}
