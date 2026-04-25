import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
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

  const checkForUpdate = useCallback(async () => {
    try {
      const info = await invoke<{
        available: boolean;
        version: string | null;
        body: string | null;
      }>("check_for_updates");

      if (info.available) {
        setState((prev) => ({
          ...prev,
          available: true,
          version: info.version,
          body: info.body,
          error: null,
        }));
      }
    } catch (e) {
      // Silent fail: update check is best-effort
      console.warn("[UpdateChecker] check failed:", e);
    }
  }, []);

  const installUpdate = useCallback(async () => {
    setState((prev) => ({ ...prev, installing: true, error: null }));
    try {
      const update = await check();
      if (update) {
        await update.downloadAndInstall();
        await relaunch();
      }
    } catch (e) {
      setState((prev) => ({
        ...prev,
        installing: false,
        error: e instanceof Error ? e.message : String(e),
      }));
    }
  }, []);

  const dismiss = useCallback(() => {
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
