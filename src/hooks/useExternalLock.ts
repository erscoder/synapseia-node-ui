import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * Snapshot of `~/.synapseia/node.lock` as reported by the Rust
 * `check_external_lock` command. `null` means no lock file (or unparseable);
 * the UI should hide the banner.
 *
 * `source` is normalised on the Rust side: anything other than "cli" or "ui"
 * collapses to "unknown". Treat unknown like cli in the UI (same kill path).
 */
export type ExternalLockInfo = {
  pid: number;
  source: "cli" | "ui" | "unknown";
  startedAt: string;
  ageSeconds: number;
  isAlive: boolean;
};

/**
 * Polls `check_external_lock` every `pollMs` and returns the latest snapshot.
 * `refresh()` is provided so callers can force a re-poll after a Start /
 * Take-over / Clean action without waiting for the next tick.
 *
 * Lifecycle:
 *   - Initial fetch on mount.
 *   - Interval until unmount.
 *   - Cleanup clears the interval AND prevents stale `setLock` writes after
 *     the component is gone (the `cancelled` flag).
 *
 * Implementation note: the interval polls a CHEAP Rust command (one FS stat,
 * one JSON parse, one syscall). It is safe to leave running for the whole
 * session.
 */
export function useExternalLock(pollMs: number = 3000): {
  lock: ExternalLockInfo | null;
  refresh: () => Promise<void>;
} {
  const [lock, setLock] = useState<ExternalLockInfo | null>(null);
  const cancelledRef = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const result = await invoke<ExternalLockInfo | null>("check_external_lock");
      if (!cancelledRef.current) {
        setLock(result ?? null);
      }
    } catch (err) {
      // The command is side-effect-free; a failure means we don't know the
      // current state. Drop the banner so the user isn't stuck staring at a
      // possibly-stale warning, and log for diagnostics.
      console.warn("[useExternalLock] check_external_lock failed:", err);
      if (!cancelledRef.current) {
        setLock(null);
      }
    }
  }, []);

  useEffect(() => {
    cancelledRef.current = false;
    void refresh();
    const id = setInterval(() => {
      void refresh();
    }, pollMs);
    return () => {
      cancelledRef.current = true;
      clearInterval(id);
    };
  }, [pollMs, refresh]);

  return { lock, refresh };
}
