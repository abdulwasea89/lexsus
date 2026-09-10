import { useEffect, useState } from "react";
import {
  ActivityIcon,
  BrainIcon,
  FolderOpenIcon,
  GitBranchIcon,
  GlobeIcon,
  MessageCircleIcon,
  MoonIcon,
  PanelLeftCloseIcon,
  PanelLeftIcon,
  SunIcon,
} from "lucide-react";
import { toggleTheme, useTheme } from "../hooks/useTheme";
import type { McpStatus } from "../lib/types";
import { cn } from "../lib/utils";

export type View = "trace" | "git" | "handoff" | "memory" | "bridge";

const RAIL_KEY = "lexsus.railOpen";

const NAV: { view: View; label: string; icon: typeof ActivityIcon }[] = [
  { view: "trace", label: "Live activity trace", icon: ActivityIcon },
  { view: "git", label: "Git", icon: GitBranchIcon },
  { view: "handoff", label: "Handoff", icon: MessageCircleIcon },
  { view: "memory", label: "Project memory", icon: BrainIcon },
  { view: "bridge", label: "Web-AI connector", icon: GlobeIcon },
];

interface WorkbenchRailProps {
  view: View;
  onViewChange: (view: View) => void;
  connector: McpStatus | null;
  onOpenProject: () => void;
}

/**
 * Left rail of the workbench: a collapsible sidebar. Icons sit in a
 * fixed-width slot that matches the collapsed rail, so they never move
 * while the rail animates — the labels slide in beside them. The
 * hover/selected highlight is an inset pill that slides in from the
 * edges in sync with the rail width.
 */
export default function WorkbenchRail({
  view,
  onViewChange,
  connector,
  onOpenProject,
}: WorkbenchRailProps) {
  const [open, setOpen] = useState(
    () => localStorage.getItem(RAIL_KEY) === "1",
  );
  const theme = useTheme();

  useEffect(() => {
    localStorage.setItem(RAIL_KEY, open ? "1" : "0");
  }, [open]);

  /** Label reveal: 0fr → 1fr grid track + fade, synced with the rail width. */
  const reveal = cn(
    "grid min-w-0 flex-1 overflow-hidden whitespace-nowrap text-xs font-medium text-left transition-[grid-template-columns,opacity] duration-300 ease-out",
    open ? "grid-cols-[1fr] opacity-100" : "grid-cols-[0fr] opacity-0",
  );
  const revealInner = "col-start-1 row-start-1 min-w-0 truncate pr-3";

  /** Fixed icon slot: exactly the collapsed rail width, so the icon
   *  position is identical whether the rail is open or closed. */
  const iconSlot =
    "relative z-10 flex w-13 shrink-0 items-center justify-center [&_svg]:size-4";

  /**
   * Highlight pill behind a row: always inset with a margin, so it reads
   * as a rounded square behind the icon when collapsed and a pill over
   * icon + label when expanded.
   */
  const pill = "absolute inset-y-0 left-2 right-2 rounded-lg";

  const rowBase =
    "relative flex h-9 w-full items-center justify-start px-0 outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50";

  function renderRow({
    key,
    label,
    icon: Icon,
    active = false,
    onClick,
  }: {
    key: string;
    label: string;
    icon: typeof ActivityIcon;
    active?: boolean;
    onClick: () => void;
  }) {
    return (
      <button
        key={key}
        type="button"
        aria-label={label}
        aria-current={active ? "page" : undefined}
        onClick={onClick}
        className={cn(
          rowBase,
          "group text-muted-foreground hover:text-foreground",
          active && "text-foreground",
        )}
      >
        <span
          aria-hidden
          className={cn(
            pill,
            active ? "bg-muted" : "bg-transparent group-hover:bg-muted/60",
          )}
        />
        <span className={iconSlot}>
          <Icon />
        </span>
        <span className={cn(reveal, "relative z-10")}>
          <span className={revealInner}>{label}</span>
        </span>
      </button>
    );
  }

  return (
    <nav
      className={cn(
        "glass-sidebar z-10 flex shrink-0 flex-col border-r py-3",
        "transition-[width] duration-300 ease-[cubic-bezier(0.2,0.8,0.2,1)]",
        open ? "w-56" : "w-13",
      )}
    >
      {/* Toggle: plain icon once the sidebar is open — no background. */}
      <button
        type="button"
        aria-label={open ? "Collapse sidebar" : "Expand sidebar"}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className={cn(
          rowBase,
          "group mb-2 text-muted-foreground hover:text-foreground",
        )}
      >
        {!open && (
          <span
            aria-hidden
            className={cn(pill, "bg-transparent group-hover:bg-muted/60")}
          />
        )}
        <span className={iconSlot}>
          {open ? <PanelLeftCloseIcon /> : <PanelLeftIcon />}
        </span>
      </button>

      <div className="flex flex-col gap-0.5">
        {NAV.map(({ view: v, label, icon }) =>
          renderRow({
            key: v,
            label,
            icon,
            active: view === v,
            onClick: () => onViewChange(v),
          }),
        )}
      </div>

      <div className="grow" />

      <div className="flex flex-col gap-0.5">
        {renderRow({
          key: "theme",
          label: theme === "dark" ? "Dark theme" : "Light theme",
          icon: theme === "dark" ? MoonIcon : SunIcon,
          onClick: toggleTheme,
        })}

        {renderRow({
          key: "project",
          label: "Project & connector",
          icon: FolderOpenIcon,
          onClick: onOpenProject,
        })}

        <span
          aria-label={
            connector?.listening
              ? "MCP connector listening"
              : "MCP connector offline"
          }
          className="relative flex h-9 w-full items-center text-xs text-muted-foreground"
        >
          <span className={iconSlot}>
            <span
              className={cn(
                "size-2 rounded-full",
                connector?.listening
                  ? connector.allow_write
                    ? "bg-warning"
                    : "bg-success"
                  : "bg-muted-foreground/40",
              )}
            />
          </span>
          <span className={cn(reveal, "relative z-10 font-normal")}>
            <span className={revealInner}>
              {connector?.listening
                ? connector.allow_write
                  ? "Connector · read/write"
                  : "Connector · read-only"
                : "Connector offline"}
            </span>
          </span>
        </span>
      </div>
    </nav>
  );
}
