import { useEffect, useRef, useState, type ReactNode } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Chip } from "@heroui/react";
import {
  ActivityIcon,
  BookOpenIcon,
  CheckIcon,
  CircleIcon,
  FileIcon,
  FlaskConicalIcon,
  PlayIcon,
  SquarePenIcon,
} from "lucide-react";
import type { FsEvent, TraceStep } from "../lib/types";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { ViewShell } from "./ViewShell";

interface TraceItem extends TraceStep {
  confirmed: boolean;
  id: number;
}

const ICONS: Record<string, ReactNode> = {
  reading: <BookOpenIcon />,
  editing: <SquarePenIcon />,
  running: <PlayIcon />,
  test: <FlaskConicalIcon />,
  error: <CircleIcon className="text-danger" />,
  fs: <FileIcon />,
};

/** Live activity trace view: web-AI tool steps + watcher grounding.
 *  Newest 3 expanded, older collapse into a summary line you can click to
 *  expand. */
export default function TraceView() {
  const [items, setItems] = useState<TraceItem[]>([]);
  const [collapsed, setCollapsed] = useState(true);
  const idRef = useRef(0);

  useEffect(() => {
    // `listen` registers asynchronously, and this view mounts/unmounts on tab
    // switch (and is double-mounted by StrictMode in dev). If it unmounts
    // before all three registrations resolve, the cleanup runs with an empty
    // array and the late-arriving listeners are never removed — a leak that
    // keeps firing setState on every visit. Register incrementally and, the
    // moment the effect is disposed, drop anything that lands after cleanup.
    let disposed = false;
    const unlistens: UnlistenFn[] = [];
    const flush = () => {
      for (const u of unlistens) u();
      unlistens.length = 0;
    };
    void (async () => {
      unlistens.push(
        await listen<TraceStep>("trace://step", (e) => {
          if (disposed) return;
          const step = e.payload;
          setItems((prev) => [
            ...prev,
            {
              ...step,
              id: ++idRef.current,
              confirmed: step.kind === "editing" ? false : step.confirmed,
            },
          ]);
        }),
      );
      if (disposed) {
        flush();
        return;
      }
      unlistens.push(
        await listen<{ path: string }>("trace://confirm", (e) => {
          if (disposed) return;
          setItems((prev) =>
            prev.map((it) =>
              it.kind === "editing" && it.file === e.payload.path
                ? { ...it, confirmed: true }
                : it,
            ),
          );
        }),
      );
      if (disposed) {
        flush();
        return;
      }
      unlistens.push(
        await listen<FsEvent>("fs://event", () => {
          if (disposed) return;
          setItems((prev) => [
            ...prev,
            {
              kind: "fs",
              file: null,
              command: null,
              detail: null,
              confirmed: false,
              agent: "watcher",
              ts: Date.now(),
              id: ++idRef.current,
            },
          ]);
        }),
      );
      if (disposed) flush();
    })();
    return () => {
      disposed = true;
      flush();
    };
  }, []);

  // Keep the list bounded (last 500).
  const visible = items.slice(-500);
  const expanded = collapsed ? visible.slice(-3) : visible;
  const earlier = visible.slice(0, Math.max(0, visible.length - 3));
  const filesTouched = new Set(
    earlier.filter((e) => e.kind === "editing").map((e) => e.file),
  ).size;

  function renderItem(it: TraceItem) {
    const icon = ICONS[it.kind] ?? <CircleIcon />;
    const label =
      it.kind === "fs"
        ? "file changed on disk"
        : it.kind === "test" || it.kind === "error"
          ? (it.detail ?? "")
          : it.file ?? it.command ?? "";
    return (
      <li
        key={it.id}
        className="flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-muted/40"
      >
        <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground [&_svg]:size-3.5">
          {icon}
        </span>
        <span className="min-w-0 flex-1 truncate text-xs">{label}</span>
        {it.kind === "editing" &&
          (it.confirmed ? (
            <Badge className="border-success/30 bg-success/10 text-success">
              <CheckIcon /> saved
            </Badge>
          ) : (
            <Badge variant="outline" className="text-warning">
              waiting
            </Badge>
          ))}
        {it.kind === "running" && (
          <code className="hidden max-w-40 truncate rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground xl:block">
            {it.command}
          </code>
        )}
        <Chip
          size="sm"
          variant="soft"
          color={it.agent === "watcher" ? "default" : "accent"}
          className="shrink-0"
        >
          {it.agent}
        </Chip>
      </li>
    );
  }

  return (
    <ViewShell
      icon={ActivityIcon}
      title="Live activity trace"
      description={`grounded against the filesystem watcher · ${visible.length} events`}
    >
      {visible.length === 0 ? (
        <div className="flex flex-1 items-center justify-center p-8 text-center">
          <div className="flex flex-col items-center gap-2">
            <ActivityIcon className="size-8 text-muted-foreground/50" />
            <p className="text-sm font-medium">No activity yet</p>
            <p className="max-w-56 text-xs leading-relaxed text-muted-foreground">
              Let a web AI work on the project — its read, write and run steps
              appear here as they execute.
            </p>
          </div>
        </div>
      ) : (
        <ul className="flex flex-col gap-0.5">
          {collapsed && earlier.length > 0 && (
            <li>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setCollapsed(false)}
                className="text-muted-foreground"
              >
                {earlier.length} earlier steps · {filesTouched} files touched
              </Button>
            </li>
          )}
          {expanded.map(renderItem)}
          {!collapsed && (
            <li>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setCollapsed(true)}
                className="text-muted-foreground"
              >
                collapse older steps
              </Button>
            </li>
          )}
        </ul>
      )}
    </ViewShell>
  );
}
