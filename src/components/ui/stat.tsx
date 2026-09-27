import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface StatProps {
  label: string;
  value: ReactNode;
  /** Optional smaller line under the number (a split, a span, a hint). */
  sub?: ReactNode;
  /** Small icon in the corner — gives the KPI strip a visual anchor. */
  icon?: LucideIcon;
  className?: string;
}

/**
 * A dashboard KPI tile: a label over a big number, with an optional sub-line
 * and icon. Lifts subtly on hover so the strip feels alive, not static.
 */
export function Stat({ label, value, sub, icon: Icon, className }: StatProps) {
  return (
    <div
      className={cn(
        "group flex min-w-0 flex-col gap-1 rounded-xl border border-border/60 bg-surface-2/40 px-3 py-2.5",
        "transition-[transform,background-color,box-shadow] duration-200 ease-out",
        "hover:-translate-y-0.5 hover:bg-surface-2/70 hover:shadow-sm",
        className,
      )}
    >
      <span className="flex items-center justify-between gap-2">
        <span className="truncate text-[11px] font-medium text-muted-foreground">
          {label}
        </span>
        {Icon && (
          <Icon className="size-3.5 shrink-0 text-muted-foreground/70 transition-colors group-hover:text-primary" />
        )}
      </span>
      <span className="truncate text-2xl font-semibold tabular-nums tracking-tight">
        {value}
      </span>
      {sub && (
        <span className="truncate text-[11px] text-muted-foreground">{sub}</span>
      )}
    </div>
  );
}
