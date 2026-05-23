import "./App.css";
import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Sidebar } from "./components/Sidebar";
import { UnlockScreen } from "./components/UnlockScreen";
import { CreateNodeScreen } from "./components/CreateNodeScreen";
import { ActivationScreen } from "./components/ActivationScreen";
import { MyNodePanel } from "./components/MyNodePanel";
import { WalletPanel } from "./components/WalletPanel";
import { StakePanel } from "./components/StakePanel";
import { SystemPanel } from "./components/SystemPanel";
import { SettingsPanel } from "./components/SettingsPanel";
import { LogViewer } from "./components/LogViewer";
import { UpdateBanner } from "./components/UpdateBanner";
import { BetaLimitModal } from "./components/BetaLimitModal";
import { CliUpdateOverlay } from "./components/CliUpdateOverlay";
import {
  PythonDepsProgress,
  subscribePythonInstall,
  upsertPhase,
  type PythonInstallProgress,
  type PythonInstallStatus,
} from "./components/PythonDepsProgress";
import { useUpdateChecker } from "./hooks/useUpdateChecker";
import { useExternalLock } from "./hooks/useExternalLock";
import { LockBanner } from "./components/LockBanner";

export type Panel = "my-node" | "wallet" | "stake" | "system" | "settings" | "logs";

export interface LogLine {
  timestamp: string;
  level: string;
  message: string;
}

export interface CommandResult {
  success: boolean;
  output: string;
  error: string | null;
}

export interface NodeStatus {
  running: boolean;
  peer_id: string | null;
  tier: number | null;
  wallet: string | null;
  balance_sol: number | null;
  balance_syn: number | null;
  staked_syn: number | null;
  pid: number | null;
}

interface WalletExistsResult {
  exists: boolean;
  wallet_path: string;
  config_exists: boolean;
}

export interface CapacityResponse {
  limit: number;
  current: number;
  accepting: boolean;
}

export interface ChainInfo {
  wallet: string | null;
  sol: number;
  syn: number;
  staked: number;
  rewardsPending: number;
  stakeAccountExists: boolean;
  stakeLockedUntil: number;
  tokenAccountExists: boolean;
  coordinatorReachable: boolean;
  vaultClaimableSyn: number;
  rewardsByType: Record<string, number>;
  presencePoints: number;
  totalWins: number;
  totalSubmissions: number;
  unclaimedSyn: number;
  totalClaimedSyn: number;
  canaryStrikes: number;
  anomalyWarnings: number;
  attestationFailures: number;
  tier: number | null;
  nodeName: string | null;
  // Optional: coord derives this server-side from `createdAt < BETA_SNAPSHOT_TS`.
  // Older coord versions don't ship the field, so the desktop treats `undefined`
  // as "not a beta tester" instead of breaking the UI.
  isBetaTester?: boolean;
}

type BootPhase = "checking" | "needs-create" | "needs-unlock" | "unlocked" | "needs-activation" | "installing-node";

function AppInner() {
  const [bootPhase, setBootPhase] = useState<BootPhase>("checking");
  const [password, setPassword] = useState<string | null>(null);
  const [walletAddress, setWalletAddress] = useState<string | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);
  const [installProgress, setInstallProgress] = useState<string>("");
  const [activePanel, setActivePanel] = useState<Panel>("my-node");
  const mainRef = useRef<HTMLElement | null>(null);
  // Re-entrancy guard for handleStartNode: a double-click on the Start button
  // (or a stale React event firing twice) must not spawn two parallel
  // `start_node`/`install_synapseia_node` flows. The Rust side has its own
  // mutex on the install path; this ref short-circuits at the UI boundary so
  // we don't even round-trip to the backend.
  const installingRef = useRef(false);
  // Re-entrancy guard for `install_python_deps`: React 18 StrictMode runs
  // effects twice in dev, and we must not double-invoke the Rust command
  // (which would race two pip processes against the same venv).
  const pythonInstallTriggeredRef = useRef(false);
  // Live phase stream from the Rust side. `terminal` is set when the
  // backend emits the final `complete` event — that's the signal to route
  // forward past the loading screen.
  const [pythonPhases, setPythonPhases] = useState<PythonInstallProgress[]>([]);
  const [pythonTerminal, setPythonTerminal] = useState<PythonInstallStatus | null>(null);

  // Reset scroll to the top whenever the active panel changes. Without this
  // the <main> container keeps its previous scrollTop, so switching into a
  // long page (Stake, Wallet) can land mid-screen and the user swears the
  // header is missing. `useLayoutEffect` not needed — the next tick is fine
  // and keeps the reset off the render-critical path.
  useEffect(() => {
    if (mainRef.current) mainRef.current.scrollTop = 0;
  }, [activePanel]);
  const [nodeStatus, setNodeStatus] = useState<NodeStatus>({
    running: false,
    peer_id: null,
    tier: null,
    wallet: null,
    balance_sol: null,
    balance_syn: null,
    staked_syn: null,
    pid: null,
  });
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [chainInfo, setChainInfo] = useState<ChainInfo | null>(null);
  // Closed-beta capacity gate. `limit`/`current` are 0 when the modal
  // was opened by the stdout-marker fallback (the CLI doesn't echo
  // numbers, only the marker line).
  const [betaLimitModal, setBetaLimitModal] = useState<{
    open: boolean;
    limit: number;
    current: number;
  }>({ open: false, limit: 0, current: 0 });
  const update = useUpdateChecker();
  // Lock-conflict banner state. Polls ~/.synapseia/node.lock every 3 s so
  // operators see immediate feedback when a CLI-spawned node (or a zombie
  // lock file) is preventing Start from working. Refresh manually after
  // any take-over / clean / start so the banner reflects reality without
  // waiting for the next poll tick.
  const { lock: externalLock, refresh: refreshLock } = useExternalLock(3000);
  // Surface non-CLI-missing start_node errors so they don't fall into
  // console.error silently — race conditions can hit start_node with a
  // lock that appeared BETWEEN check_external_lock polls. The banner is
  // the primary signal; this is the inline fallback.
  const [startError, setStartError] = useState<string | null>(null);

  // Decide at boot whether we're creating a wallet or unlocking one.
  // Step 1 ensures the @synapseia-network/node CLI is on disk BEFORE the
  // wallet branch — `install_synapseia_node` is idempotent (early-returns
  // with an `already-installed` event when present), so the spinner is a
  // sub-100ms flash on the common path. If install fails we fall through
  // to `checking` and rely on handleStartNode's ERR_CLI_MISSING fallback.
  useEffect(() => {
    (async () => {
      try {
        setBootPhase("installing-node");
        setInstallProgress("Checking @synapseia-network/node CLI…");
        await invoke<string>("install_synapseia_node");
      } catch (e) {
        // Install failure must not strand the user: surface the error,
        // log it, and fall through to the wallet check. handleStartNode
        // will retry the install when the user clicks Start.
        const msg = e instanceof Error ? e.message : String(e);
        console.warn("[boot] install_synapseia_node failed:", msg);
        setInstallError(msg);
      }

      // Kick off Python deps install (idempotent on the Rust side; emits
      // `python-install-progress` events that drive the in-screen list).
      // Don't block the wallet check on it here — the parent effect below
      // gates `setBootPhase` on `pythonTerminal !== null` so the loading
      // screen stays visible until the stream ends.
      if (!pythonInstallTriggeredRef.current) {
        pythonInstallTriggeredRef.current = true;
        try {
          await invoke<string>("install_python_deps");
        } catch (e) {
          // Tauri command failures (channel error, command missing, etc.)
          // mark the section as errored so the user sees feedback even
          // when the backend never gets to emit a `complete` event.
          const msg = e instanceof Error ? e.message : String(e);
          console.warn("[boot] install_python_deps failed:", msg);
          setPythonPhases((prev) =>
            upsertPhase(prev, { phase: "complete", status: "error", message: msg }),
          );
          setPythonTerminal("error");
        }
      }
    })();
  }, []);

  // Subscribe to the python-install-progress stream once. Lives in its own
  // effect so it stays mounted across StrictMode double-invokes.
  useEffect(() => {
    const unlisten = subscribePythonInstall((payload) => {
      // `complete` and `skipped` are terminal signals — capture the final
      // status and don't render them as their own row (the rows above
      // already show every phase's outcome). `skipped` on its own (without
      // any prior rows) is the "nothing to do" path; PythonDepsProgress
      // detects that and renders a single one-liner.
      if (payload.phase === "complete" || payload.phase === "skipped") {
        setPythonTerminal(payload.status);
        return;
      }
      setPythonPhases((prev) => upsertPhase(prev, payload));
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Gate the transition out of `installing-node` on the python install
  // terminal signal. Once `pythonTerminal` is set (done/error/skip) we
  // move forward — `error` still routes forward so a missing Python venv
  // doesn't lock the user out (some WO types just won't be available).
  useEffect(() => {
    if (bootPhase !== "installing-node") return;
    if (pythonTerminal === null) return;
    (async () => {
      try {
        const info = await invoke<WalletExistsResult>("wallet_exists");
        setBootPhase(info.exists ? "needs-unlock" : "needs-create");
      } catch (e) {
        setBootError(e instanceof Error ? e.message : String(e));
        setBootPhase("needs-create");
      }
    })();
  }, [bootPhase, pythonTerminal]);

  useEffect(() => {
    const unlisten = listen<LogLine>("node-log", (event) => {
      setLogs((prev) => {
        const next = [...prev, event.payload];
        return next.slice(-2000);
      });
      // Beta-limit fallback: catches the race window between the
      // pre-flight `check_capacity` probe and the CLI's first
      // heartbeat. The node CLI emits this marker (slice S2) when the
      // coordinator rejects with HTTP 403 + BETA_LIMIT_REACHED. No
      // parsed numbers — just the marker.
      const msg = event.payload?.message ?? "";
      if (/^\[BETA_LIMIT_REACHED\]/.test(msg)) {
        setBetaLimitModal({ open: true, limit: 0, current: 0 });
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<{ phase: string; message: string }>("install-progress", (event) => {
      setInstallProgress(event.payload.message);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Runtime-only fields come from the Rust side (running, pid).
  useEffect(() => {
    if (!nodeStatus.running) return;
    const interval = setInterval(async () => {
      try {
        const status = await invoke<NodeStatus>("node_status");
        setNodeStatus((prev) => ({ ...prev, running: status.running, pid: status.pid }));
      } catch (e) {
        console.error("Failed to get node status", e);
      }
    }, 5000);
    return () => clearInterval(interval);
  }, [nodeStatus.running]);

  // One refresh function, shared with every panel that has a Refresh
  // button. Child panels used to `invoke("fetch_chain_info")` directly,
  // which hit the Rust backend but never propagated the result up here —
  // so nothing on screen updated until the next 15 s poll tick.
  //
  // Returns the freshly fetched ChainInfo (or null on failure). Most callers
  // ignore the return value and rely on the `chainInfo` state update, but
  // StakePanel needs the fresh value SYNCHRONOUSLY to poll on-chain
  // confirmation after a stake/unstake — its `chainInfo` prop won't update
  // mid-async-function, so it reads what this returns instead of the stale
  // closure value.
  const refreshChainInfo = useCallback(async (): Promise<ChainInfo | null> => {
    try {
      const info = await invoke<ChainInfo>("fetch_chain_info");
      setChainInfo(info);
      setNodeStatus((prev) => ({
        ...prev,
        wallet: info.wallet ?? prev.wallet,
        tier: info.tier ?? prev.tier,
        balance_sol: info.sol,
        balance_syn: info.syn,
        staked_syn: info.staked,
      }));
      return info;
    } catch (e) {
      console.warn("[refresh] fetch_chain_info failed:", e);
      return null;
    }
  }, []);

  // Chain/wallet fields come from the lightweight `chain-info` helper on
  // the Rust side (NestJS-free, no coordinator heartbeat). The poll just
  // delegates to refreshChainInfo so there's a single source of truth.
  useEffect(() => {
    if (bootPhase !== "unlocked" && bootPhase !== "needs-activation") return;
    let cancelled = false;
    const tick = async () => {
      if (cancelled) return;
      await refreshChainInfo();
    };
    tick();
    const id = setInterval(tick, 15000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [bootPhase, refreshChainInfo]);

  const handleUnlock = useCallback((pwd: string, walletAddr: string) => {
    setPassword(pwd);
    setWalletAddress(walletAddr);
    // Advance immediately so the UnlockScreen unmounts — otherwise its
    // "Unlocking..." spinner stays visible while the balance check runs.
    setBootPhase("unlocked");

    // Background activation check via the lightweight chain-info helper.
    // Fail SAFE: only route to ActivationScreen when we have POSITIVE
    // evidence the wallet is unactivated (SOL came back as 0 on a
    // successful RPC call). Any RPC hiccup keeps the user on the dashboard.
    (async () => {
      try {
        const info = await invoke<ChainInfo>("fetch_chain_info");
        if (info.sol < 0.001 && !info.tokenAccountExists) {
          setBootPhase("needs-activation");
        }
      } catch (e) {
        console.warn("[unlock] background chain-info check failed:", e);
      }
    })();
  }, []);

  const handleCreated = useCallback((pwd: string, walletAddr: string) => {
    setPassword(pwd);
    setWalletAddress(walletAddr);
    // Brand-new wallets need a small SOL deposit before they can earn rewards.
    setBootPhase("needs-activation");
  }, []);

  const handleStartNode = useCallback(async () => {
    if (installingRef.current) return;
    if (!password) return;
    // F-node-ui-017 (P14): claim the re-entrancy guard UNCONDITIONALLY at
    // the top of the function and release it from a single `finally` at
    // the bottom. The previous shape set the flag *after* the capacity
    // pre-flight (a multi-second network call), so a frantic
    // double-click during that window could spawn two `start_node`
    // invocations in parallel — both would observe `installingRef.current
    // === false` because the flag hadn't been claimed yet.
    installingRef.current = true;
    // Clear any prior start error so the user sees a fresh attempt.
    setStartError(null);
    try {
      // Pre-flight capacity probe. We hit the coordinator's public
      // `/peer/capacity` endpoint BEFORE spawning the CLI; on a
      // closed-beta full house we surface the modal instantly without
      // the multi-second cold-start of the node binary. Network errors
      // are NOT treated as a beta-limit signal — false positives are
      // worse than the small race window we have anyway. Errors fall
      // through to `start_node`, which surfaces them through the
      // existing error path / log viewer.
      try {
        const cap = await invoke<CapacityResponse>("check_capacity");
        if (!cap.accepting) {
          setBetaLimitModal({ open: true, limit: cap.limit, current: cap.current });
          return;
        }
      } catch (err) {
        console.warn("check_capacity failed; proceeding to start_node", err);
      }

      try {
        const status = await invoke<NodeStatus>("start_node", { password });
        setNodeStatus(status);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        if (msg.includes("ERR_CLI_MISSING")) {
          setBootPhase("installing-node");
          setInstallError(null);
          setInstallProgress("Preparing installer…");
          try {
            await invoke<string>("install_synapseia_node");
            const status = await invoke<NodeStatus>("start_node", { password });
            setNodeStatus(status);
            setBootPhase("unlocked");
          } catch (installErr) {
            const ierr = installErr instanceof Error ? installErr.message : String(installErr);
            setInstallError(ierr);
          }
          return;
        }
        // Generic non-CLI-missing errors (already-running, lock-claimed,
        // capacity rejection from the CLI itself, etc). Surface to the UI —
        // do NOT swallow into console.error only, that was the original bug.
        console.error("Failed to start node", e);
        setStartError(msg);
        // Lock state may have flipped under us; refresh the banner.
        void refreshLock();
      }
    } finally {
      installingRef.current = false;
    }
  }, [password, refreshLock]);

  /**
   * Force-release the external lock (alive case: SIGTERM + SIGKILL fallback;
   * dead case: just delete the file) and refresh the banner.
   * The Rust side re-verifies the lock owner before SIGKILL — see P2 fail-
   * closed identity check in commands.rs::force_release_lock.
   */
  const handleTakeOver = useCallback(async () => {
    try {
      await invoke<void>("force_release_lock");
      setStartError(null);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      console.error("Failed to release lock", e);
      setStartError(msg);
    } finally {
      await refreshLock();
    }
  }, [refreshLock]);

  /** Same backend path as take-over — the command handles both alive and
   *  dead PIDs. Kept as a distinct handler so future UX changes (e.g.
   *  different toast on success) can diverge cleanly. */
  const handleCleanStale = useCallback(async () => {
    await handleTakeOver();
  }, [handleTakeOver]);

  const handleStopNode = useCallback(async () => {
    try {
      await invoke("stop_node");
      setNodeStatus((prev) => ({ ...prev, running: false, pid: null }));
    } catch (e) {
      console.error("Failed to stop node", e);
    }
  }, []);

  if (bootPhase === "checking") {
    return (
      <div className="flex flex-col items-center justify-center min-h-screen bg-[var(--bg-primary)] text-slate-400">
        <p>Checking wallet…</p>
        {bootError && <p className="text-red-400 text-xs mt-2">{bootError}</p>}
      </div>
    );
  }

  if (bootPhase === "installing-node") {
    return (
      <div className="flex flex-col items-center justify-center min-h-screen bg-[var(--bg-primary)] text-slate-200 px-6">
        <div className="flex flex-col items-center gap-4 max-w-md">
          <div className="h-12 w-12 rounded-full border-2 border-slate-600 border-t-emerald-400 animate-spin" />
          <h1 className="text-lg font-semibold">Setting up Synapseia node</h1>
          <p className="text-sm text-slate-400 text-center">{installProgress || "Installing @synapseia-network/node CLI from npm…"}</p>
          <PythonDepsProgress phases={pythonPhases} terminal={pythonTerminal} />
          {installError && (
            <div className="mt-4 w-full rounded border border-red-500/40 bg-red-500/10 p-4 text-xs text-red-300 whitespace-pre-line">
              <p className="font-semibold mb-1">Installation failed</p>
              <p>{installError}</p>
              <button
                onClick={async () => {
                  setInstallError(null);
                  setBootPhase("checking");
                  try {
                    const info = await invoke<WalletExistsResult>("wallet_exists");
                    setBootPhase(info.exists ? "needs-unlock" : "needs-create");
                  } catch (e) {
                    setBootError(e instanceof Error ? e.message : String(e));
                    setBootPhase("needs-create");
                  }
                }}
                className="mt-3 rounded bg-slate-700 px-3 py-1 text-xs text-slate-100 hover:bg-slate-600"
              >
                Dismiss
              </button>
            </div>
          )}
        </div>
      </div>
    );
  }

  // Surfaced below CreateNodeScreen/UnlockScreen when Python deps install
  // hit an error. We don't block the user (some WO types just won't be
  // available) but they deserve a heads-up + the retry path.
  const pythonWarning =
    pythonTerminal === "error" ? (
      <div className="mx-auto mt-3 max-w-md rounded border border-amber-500/40 bg-amber-500/10 px-4 py-2 text-xs text-amber-200">
        Python deps install had errors. Some work order types may not be available. Re-run from
        the menu to retry.
      </div>
    ) : null;

  if (bootPhase === "needs-create") {
    return (
      <>
        <CreateNodeScreen onCreated={handleCreated} />
        {pythonWarning}
      </>
    );
  }

  if (bootPhase === "needs-unlock") {
    return (
      <>
        <UnlockScreen onUnlock={handleUnlock} />
        {pythonWarning}
      </>
    );
  }

  // F-node-ui-018 (STYLE): the BetaLimitModal was previously rendered
  // twice — once in the `needs-activation` branch and again in the main
  // panel return. Lift it to a single root wrapper so the modal exists
  // exactly once regardless of `bootPhase`.
  const betaModal = (
    <BetaLimitModal
      open={betaLimitModal.open}
      limit={betaLimitModal.limit}
      current={betaLimitModal.current}
      onClose={() => setBetaLimitModal((s) => ({ ...s, open: false }))}
    />
  );

  if (bootPhase === "needs-activation" && walletAddress && password) {
    return (
      <>
        {betaModal}
        <ActivationScreen
          walletAddress={walletAddress}
          password={password}
          onActivated={() => {
            setBootPhase("unlocked");
            handleStartNode();
          }}
        />
      </>
    );
  }

  return (
    <div className="flex h-screen text-slate-100 font-mono">
      {betaModal}
      <Sidebar
        activePanel={activePanel}
        onPanelChange={setActivePanel}
        nodeRunning={nodeStatus.running}
      />
      <main ref={mainRef} className="flex-1 overflow-auto p-6">
        {update.available && (
          <div className="mb-4">
            <UpdateBanner
              version={update.version}
              installing={update.installing}
              error={update.error}
              onInstall={update.installUpdate}
              onDismiss={update.dismiss}
            />
          </div>
        )}
        {externalLock && (
          <div className="mb-4">
            <LockBanner
              lock={externalLock}
              onTakeOver={handleTakeOver}
              onCleanStale={handleCleanStale}
            />
          </div>
        )}
        {startError && !externalLock && (
          <div
            role="alert"
            className="mb-4 rounded-md border border-red-500/40 bg-red-500/10 px-4 py-3 text-sm text-red-200"
          >
            <p className="font-semibold">Could not start node</p>
            <p className="mt-1 text-xs opacity-90 whitespace-pre-line">{startError}</p>
            <button
              type="button"
              onClick={() => setStartError(null)}
              className="mt-2 rounded bg-slate-700 px-3 py-1 text-xs text-slate-100 hover:bg-slate-600"
            >
              Dismiss
            </button>
          </div>
        )}
        {activePanel === "my-node" && (
          <MyNodePanel
            password={password}
            chainInfo={chainInfo}
            status={nodeStatus}
            onStart={handleStartNode}
            onStop={handleStopNode}
            onOpenLogs={() => setActivePanel("logs")}
            onRefresh={refreshChainInfo}
            startDisabled={!!externalLock}
          />
        )}
        {activePanel === "wallet" && (
          <WalletPanel
            password={password}
            status={nodeStatus}
            chainInfo={chainInfo}
            onRefresh={refreshChainInfo}
          />
        )}
        {activePanel === "stake" && (
          <StakePanel
            password={password}
            status={nodeStatus}
            chainInfo={chainInfo}
            onRefresh={refreshChainInfo}
          />
        )}
        {activePanel === "system" && <SystemPanel />}
        {activePanel === "settings" && <SettingsPanel password={password} />}
        {activePanel === "logs" && (
          <LogViewer
            logs={logs}
            status={nodeStatus}
            password={password}
            onStart={handleStartNode}
            onStop={handleStopNode}
          />
        )}
      </main>
    </div>
  );
}

function App() {
  return (
    <>
      <AppInner />
      <CliUpdateOverlay />
    </>
  );
}

export default App;
