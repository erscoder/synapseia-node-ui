import { ReactNode } from "react";
import { clsx } from "clsx";

export type PillTone = "green" | "cyan" | "purple" | "amber" | "red" | "slate";

interface StatusPillProps {
  tone?: PillTone;
  pulse?: boolean;
  children: ReactNode;
  className?: string;
}

const toneClass: Record<PillTone, string> = {
  green: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30",
  cyan: "bg-cyan-500/15 text-cyan-300 border-cyan-500/30",
  purple: "bg-purple-500/15 text-purple-300 border-purple-500/30",
  amber: "bg-amber-500/15 text-amber-300 border-amber-500/30",
  red: "bg-red-500/15 text-red-300 border-red-500/30",
  slate: "bg-slate-500/15 text-slate-300 border-slate-500/30",
};

const dotClass: Record<PillTone, string> = {
  green: "bg-emerald-400",
  cyan: "bg-cyan-400",
  purple: "bg-purple-400",
  amber: "bg-amber-400",
  red: "bg-red-400",
  slate: "bg-slate-400",
};

export function StatusPill({ tone = "slate", pulse = false, children, className }: StatusPillProps) {
  return (
    <span
      className={clsx(
        "inline-flex items-center gap-2 px-3 py-1 rounded-full border text-xs font-medium",
        toneClass[tone],
        className,
      )}
    >
      <span
        className={clsx(
          "w-1.5 h-1.5 rounded-full",
          dotClass[tone],
          pulse && "animate-pulse",
        )}
      />
      {children}
    </span>
  );
}
