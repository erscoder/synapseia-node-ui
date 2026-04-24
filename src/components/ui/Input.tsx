import { InputHTMLAttributes, forwardRef } from "react";
import { clsx } from "clsx";

interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  hint?: string;
  error?: string;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { label, hint, error, className, id, ...rest },
  ref,
) {
  const inputId = id ?? rest.name;
  return (
    <div className="space-y-1.5">
      {label && (
        <label htmlFor={inputId} className="block text-xs uppercase tracking-wide text-slate-400">
          {label}
        </label>
      )}
      <input
        ref={ref}
        id={inputId}
        className={clsx(
          "w-full px-4 py-2.5 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06]",
          "rounded-lg text-slate-100 placeholder-slate-500 transition-all",
          "focus:outline-none focus:border-[var(--accent-cyan)]/60 focus:ring-2 focus:ring-[var(--accent-cyan)]/20",
          "disabled:opacity-60",
          error && "border-red-500/50 focus:border-red-500 focus:ring-red-500/20",
          className,
        )}
        {...rest}
      />
      {hint && !error && <p className="text-xs text-slate-500">{hint}</p>}
      {error && <p className="text-xs text-red-400">{error}</p>}
    </div>
  );
});
