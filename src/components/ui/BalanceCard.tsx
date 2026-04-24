import { Card } from "./Card";

export type BalanceAccent = "cyan" | "purple" | "green" | "amber" | "blue" | "slate";

interface BalanceCardProps {
  label: string;
  value: string;
  symbol?: string;
  accent?: BalanceAccent;
  hint?: string;
}

// Accent colors match packages/dashboard's tier palette: blue for primary,
// emerald for value signals, purple for accents, amber for warnings.
const accentClass: Record<BalanceAccent, string> = {
  cyan: "text-[var(--accent-cyan)]",
  purple: "text-[var(--accent-purple)]",
  green: "text-emerald-400",
  amber: "text-amber-400",
  blue: "text-blue-400",
  slate: "text-slate-300",
};

export function BalanceCard({ label, value, symbol, accent = "slate", hint }: BalanceCardProps) {
  return (
    <Card padding="sm">
      <p className="text-xs uppercase tracking-wide text-slate-400 mb-1">{label}</p>
      <p className="text-xl font-bold text-slate-100 font-mono">
        {value}
        {symbol && <span className={`ml-1 text-sm font-medium ${accentClass[accent]}`}>{symbol}</span>}
      </p>
      {hint && <p className="text-xs text-slate-500 mt-1">{hint}</p>}
    </Card>
  );
}
