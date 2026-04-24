import { ButtonHTMLAttributes, ReactNode } from "react";
import { clsx } from "clsx";

export type ButtonVariant = "primary" | "secondary" | "danger" | "ghost";
export type ButtonSize = "sm" | "md" | "lg";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  children?: ReactNode;
}

const variantClass: Record<ButtonVariant, string> = {
  // Primary = cyan accent with subtle glow, matching packages/dashboard
  primary:
    "bg-[var(--accent-cyan)]/90 hover:bg-[var(--accent-cyan)] text-[var(--bg-primary)] shadow-lg shadow-cyan-500/20 border border-[var(--accent-cyan)]/40",
  secondary:
    "bg-[var(--bg-surface)]/80 backdrop-blur-md hover:bg-[var(--bg-elevated)] text-slate-200 border border-white/[0.06]",
  danger:
    "bg-[var(--accent-red)]/90 hover:bg-[var(--accent-red)] text-white shadow-lg shadow-red-500/20 border border-[var(--accent-red)]/40",
  ghost: "bg-transparent hover:bg-white/[0.04] text-slate-300 border border-transparent",
};

const sizeClass: Record<ButtonSize, string> = {
  sm: "px-3 py-1.5 text-xs",
  md: "px-4 py-2 text-sm",
  lg: "px-6 py-3 text-sm",
};

export function Button({
  variant = "secondary",
  size = "md",
  children,
  className,
  ...rest
}: ButtonProps) {
  return (
    <button
      className={clsx(
        "inline-flex items-center justify-center gap-2 rounded-lg font-medium transition-all",
        "disabled:opacity-40 disabled:cursor-not-allowed disabled:shadow-none",
        sizeClass[size],
        variantClass[variant],
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}
