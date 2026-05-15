import { useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Renders a non-dismissable overlay while the Tauri host is downloading or
 * upgrading the @synapseia-network/node CLI. The Rust side emits
 * `install-progress` events with phase one of:
 *   "starting" | "upgrading" | "downloading-node" | "extracting"
 *   | "installing-npm-package" | "already-installed" | "complete"
 *
 * We only render the overlay while phase is NOT `already-installed` or
 * `complete`, so the common boot path (CLI present + up-to-date) stays
 * invisible. When a stale CLI triggers the new freshness reinstall path,
 * the overlay surfaces the in-flight upgrade so the user knows not to
 * close the app mid-install.
 */
interface ProgressEvent {
  phase: string;
  message: string;
}

const TERMINAL_PHASES = new Set(["already-installed", "complete"]);

export function CliUpdateOverlay() {
  const [event, setEvent] = useState<ProgressEvent | null>(null);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      unlisten = await listen<ProgressEvent>("install-progress", (e) => {
        setEvent(e.payload);
      });
    })();
    return () => {
      unlisten?.();
    };
  }, []);

  if (!event || TERMINAL_PHASES.has(event.phase)) return null;

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/70 backdrop-blur-sm">
      <div className="max-w-md rounded-lg border border-white/10 bg-[var(--bg-elevated)] p-6 text-center shadow-2xl">
        <div className="mx-auto mb-4 h-10 w-10 animate-spin rounded-full border-2 border-emerald-400 border-t-transparent" />
        <h2 className="text-lg font-semibold text-slate-100">
          Updating node CLI
        </h2>
        <p className="mt-2 text-sm text-slate-300">{event.message}</p>
        <p className="mt-3 text-xs text-slate-500">
          Do not close the app — this only happens when a new version is available.
        </p>
      </div>
    </div>
  );
}
