import { HTMLAttributes, ReactNode } from "react";
import { clsx } from "clsx";

// Matches packages/dashboard surface: translucent base + backdrop blur + hairline
// white border. Nothing else in the tree should replicate this — prefer this
// primitive over raw Tailwind strings.
export interface CardProps extends HTMLAttributes<HTMLDivElement> {
  children?: ReactNode;
  padding?: "none" | "sm" | "md" | "lg";
  interactive?: boolean;
}

const paddingClass = {
  none: "",
  sm: "p-4",
  md: "p-6",
  lg: "p-8",
} as const;

export function Card({
  children,
  padding = "md",
  interactive = false,
  className,
  ...rest
}: CardProps) {
  return (
    <div
      className={clsx(
        "bg-[var(--bg-surface)]/70 backdrop-blur-md rounded-xl border border-white/[0.06]",
        paddingClass[padding],
        interactive && "transition-all hover:border-white/[0.12] hover:bg-[var(--bg-surface)]/90",
        className,
      )}
      {...rest}
    >
      {children}
    </div>
  );
}
