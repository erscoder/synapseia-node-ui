import { useEffect } from "react";
import { UsersRound } from "lucide-react";
import { Card, Button } from "./ui";

interface BetaLimitModalProps {
  open: boolean;
  onClose: () => void;
  /** 0 means "unknown" — happens on the stdout-marker fallback path. */
  limit: number;
  /** 0 means "unknown" — same fallback case. */
  current: number;
}

/**
 * Full-screen overlay shown the moment we know the closed-beta cap is
 * reached. Mirrors the ActivationScreen pattern (centered Card on a
 * blurred backdrop, plain Tailwind, no Radix/Headless UI).
 *
 * UX: backdrop click / ESC / Enter / OK button all close. The OK
 * button is autofocused so keyboard users dismiss with a single Enter.
 */
export function BetaLimitModal({ open, onClose, limit, current }: BetaLimitModalProps) {
  useEffect(() => {
    if (!open) return;

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === "Enter") {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  const hasNumbers = limit > 0;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-labelledby="beta-limit-title"
      onClick={onClose}
    >
      <div
        className="w-full max-w-md p-4"
        onClick={(e) => e.stopPropagation()}
      >
        <Card padding="md" className="space-y-5">
          <div className="flex flex-col items-center text-center space-y-3">
            <div className="w-14 h-14 rounded-full bg-cyan-500/10 border border-cyan-500/30 flex items-center justify-center">
              <UsersRound className="w-7 h-7 text-cyan-300" />
            </div>
            <h2
              id="beta-limit-title"
              className="text-xl font-bold text-slate-100 tracking-tight"
            >
              Beta tester limit reached
            </h2>
            <p className="text-slate-400 text-sm leading-relaxed">
              Synapseia is currently in closed beta on Solana devnet.
              {hasNumbers
                ? ` The maximum of ${limit} nodes has been reached.`
                : " The beta tester limit has been reached."}
              {" "}
              Please check back later — Synapseia will be available on
              mainnet soon.
            </p>
            {hasNumbers && current > 0 && (
              <p className="text-slate-500 text-xs">
                {current} / {limit} nodes registered
              </p>
            )}
          </div>

          <div className="flex justify-center">
            <Button
              autoFocus
              variant="primary"
              size="lg"
              onClick={onClose}
            >
              OK
            </Button>
          </div>
        </Card>
      </div>
    </div>
  );
}
