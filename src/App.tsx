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
import { useUpdateChecker } from "./hooks/useUpdateChecker";

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
}

type BootPhase = "checking" | "needs-create" | "needs-unlock" | "unlocked" | "needs-activation";

function App() {
  const [bootPhase, setBootPhase] = useState<BootPhase>("checking");
  const [password, setPassword] = useState<string | null>(null);
  const [walletAddress, setWalletAddress] = useState<string | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  const [activePanel, setActivePanel] = useState<Panel>("my-node");
  const mainRef = useRef<HTMLElement | null>(null);

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

  // Decide at boot whether we're creating a wallet or unlocking one.
  useEffect(() => {
    (async () => {
      try {
        const info = await invoke<WalletExistsResult>("wallet_exists");
        setBootPhase(info.exists ? "needs-unlock" : "needs-create");
      } catch (e) {
        setBootError(e instanceof Error ? e.message : String(e));
        setBootPhase("needs-create");
      }
    })();
  }, []);

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
  const refreshChainInfo = useCallback(async () => {
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
    } catch (e) {
      console.warn("[refresh] fetch_chain_info failed:", e);
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
    if (!password) return;
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
      console.error("Failed to start node", e);
    }
  }, [password]);

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

  if (bootPhase === "needs-create") {
    return <CreateNodeScreen onCreated={handleCreated} />;
  }

  if (bootPhase === "needs-unlock") {
    return <UnlockScreen onUnlock={handleUnlock} />;
  }

  if (bootPhase === "needs-activation" && walletAddress && password) {
    return (
      <>
        <BetaLimitModal
          open={betaLimitModal.open}
          limit={betaLimitModal.limit}
          current={betaLimitModal.current}
          onClose={() => setBetaLimitModal((s) => ({ ...s, open: false }))}
        />
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
      <BetaLimitModal
        open={betaLimitModal.open}
        limit={betaLimitModal.limit}
        current={betaLimitModal.current}
        onClose={() => setBetaLimitModal((s) => ({ ...s, open: false }))}
      />
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
        {activePanel === "my-node" && (
          <MyNodePanel
            password={password}
            chainInfo={chainInfo}
            status={nodeStatus}
            onStart={handleStartNode}
            onStop={handleStopNode}
            onOpenLogs={() => setActivePanel("logs")}
            onRefresh={refreshChainInfo}
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
        {activePanel === "logs" && <LogViewer logs={logs} />}
      </main>
    </div>
  );
}

export default App;
