import { useState, useEffect, useCallback, useRef } from "react";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export interface UpdateState {
  available: boolean;
  version: string | null;
  body: string | null;
  installing: boolean;
  error: string | null;
}

const INITIAL: UpdateState = {
  available: false,
  version: null,
  body: null,
  installing: false,
  error: null,
};

const CHECK_INTERVAL_MS = 30 * 60 * 1000; // 30 minutes

export function useUpdateChecker() {
  const [state, setState] = useState<UpdateState>(INITIAL);
  // F-node-ui-019 (PERF): mirror `installing` into a ref so the
  // periodic interval can early-exit without re-creating itself every
  // time the state flips. The interval body always reads the latest
  // value via the ref.
  const installingRef = useRef(false);

  const checkForUpdate = useCallback(async () => {
    // F-node-ui-019: never poll while an install is in flight — a
    // mid-install `check()` races the downloader inside the plugin and
    // surfaces as a confusing "update vanished" UI flicker.
    if (installingRef.current) return;
    try {
      // F-node-ui-015 (P10): call the updater plugin directly. The Rust
      // `check_for_updates` IPC wrapper has been removed.
      const update = await check();
      if (update) {
        setState((prev) => ({
          ...prev,
          available: true,
          version: update.version ?? null,
          body: update.body ?? null,
          error: null,
        }));
      }
    } catch (e) {
      // Silent fail: update check is best-effort
      console.warn("[UpdateChecker] check failed:", e);
    }
  }, []);

  const installUpdate = useCallback(async () => {
    installingRef.current = true;
    setState((prev) => ({ ...prev, installing: true, error: null }));
    try {
      const update = await check();
      if (update) {
        await update.downloadAndInstall();
        await relaunch();
      }
    } catch (e) {
      installingRef.current = false;
      setState((prev) => ({
        ...prev,
        installing: false,
        error: e instanceof Error ? e.message : String(e),
      }));
    }
  }, []);

  const dismiss = useCallback(() => {
    installingRef.current = false;
    setState(INITIAL);
  }, []);

  // Check on mount + periodic
  useEffect(() => {
    checkForUpdate();
    const id = setInterval(checkForUpdate, CHECK_INTERVAL_MS);
    return () => clearInterval(id);
  }, [checkForUpdate]);

  return { ...state, installUpdate, dismiss };
}
