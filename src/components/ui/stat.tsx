import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface StatProps {
  label: string;
  value: ReactNode;
  /** Optional smaller line under the number (a split, a span, a hint). */
  sub?: ReactNode;
  className?: string;
}

/**
 * A dashboard KPI tile: a label over a big number, with an optional sub-line.
 * Kept as its own primitive so the dashboard markup stays about the data and
 * not about the tile chrome.
 */
export function Stat({ label, value, sub, className }: StatProps) {
  return (
    <div
      className={cn(
        "flex min-w-0 flex-col gap-1 rounded-lg border border-border/60 bg-surface-2/50 px-3 py-2.5",
        className,
      )}
    >
      <span className="truncate text-[11px] font-medium text-muted-foreground">
        {label}
      </span>
      <span className="truncate text-xl font-semibold tabular-nums tracking-tight">
        {value}
      </span>
      {sub && (
        <span className="truncate text-[11px] text-muted-foreground">{sub}</span>
      )}
    </div>
  );
}
