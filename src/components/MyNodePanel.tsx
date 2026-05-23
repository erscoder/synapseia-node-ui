import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChainInfo, CommandResult, NodeStatus } from "../App";
import {
  Monitor, Trophy, Shield, Coins, Activity, RefreshCw, Play, Square, Terminal,
  CheckCircle2, AlertTriangle, XCircle, Star, Loader2, Sparkles,
} from "lucide-react";
import { Card, Button, PageHeader } from "./ui";

interface Props {
  password: string | null;
  chainInfo: ChainInfo | null;
  status: NodeStatus;
  onStart: () => void;
  onStop: () => void;
  onOpenLogs: () => void;
  // Returns the freshly fetched ChainInfo (shared with StakePanel's
  // confirmation poll). This panel awaits it and ignores the value.
  onRefresh: () => Promise<ChainInfo | null>;
  /** When the external lock banner is visible, the Start button must be
   *  disabled so the operator goes through the take-over / clean flow
   *  before retrying. Kept as a plain boolean so the panel stays
   *  ignorant of the lock-source semantics. */
  startDisabled?: boolean;
}

// Aggregated reward groups for the breakdown UI. Each group sums one or more
// backend RewardType keys (see packages/coordinator/src/domain/entities/Reward.ts).
// `order` controls render order; lower first. Mirrors the dashboard
// `packages/dashboard/app/(app)/my-node/page.tsx` Rewards card 1:1 so operators
// see the same semantic groups across web and desktop.
const REWARD_GROUPS: ReadonlyArray<{
  label: string;
  color: string;
  order: number;
  keys: ReadonlyArray<string>;
}> = [
  { label: "Research Wins",     color: "text-amber-400",   order: 1, keys: ["research"] },
  { label: "Training Rewards",  color: "text-cyan-400",    order: 2, keys: ["training", "diloco", "lora", "docking"] },
  { label: "Inference Rewards", color: "text-purple-400",  order: 3, keys: ["cpu_inference", "gpu_inference"] },
  { label: "Peer Review",       color: "text-violet-400",  order: 4, keys: ["evaluation"] },
  { label: "Work Orders",       color: "text-blue-400",    order: 5, keys: ["work_order"] },
];

// Backend RewardType keys that are intentionally NOT shown on the per-node
// Rewards card. They are "known-but-handled-elsewhere" — distinct from unknown
// keys (which fall through to the gray fallback row).
//
// - `staking`: staking rewards live in the dashboard at
//   `app.synapseia.network/staking`. The desktop app intentionally omits a
//   staking surface — surfacing the rewards here without the stake/unstake
//   flow would imply they accrue locally. Operators manage staking from the
//   web dashboard.
// - `referral`: `RewardType.REFERRAL` exists in the coordinator enum but no
//   code path actually emits it today. Rendering an always-zero row (or a
//   stray non-zero row from a future experiment) would imply a feature that
//   isn't shipped. Re-add the group here only when the referral flow is live.
const EXCLUDED_KEYS = new Set<string>(["staking", "referral"]);

// All keys covered by a known group. Any byType key NOT in this set AND NOT
// in EXCLUDED_KEYS renders as a gray fallback row so future RewardType
// additions in the coordinator don't silently disappear from the UI.
const KNOWN_REWARD_KEYS = new Set<string>(REWARD_GROUPS.flatMap((g) => g.keys));

function humanizeKey(key: string): string {
  return key
    .split("_")
    .map((part) => (part.length === 0 ? part : part[0].toUpperCase() + part.slice(1)))
    .join(" ");
}

// `byType` values arrive as numbers from the Tauri ChainInfo bridge (unlike the
// web dashboard which uses string-encoded amounts). Accept both shapes so the
// helper stays drop-in if the bridge ever flips to strings.
function sumKeys(
  byType: Record<string, number | string>,
  keys: ReadonlyArray<string>,
): number {
  let total = 0;
  for (const k of keys) {
    const raw = byType[k];
    if (raw === undefined || raw === null) continue;
    const n = typeof raw === "number" ? raw : parseFloat(raw);
    if (Number.isFinite(n)) total += n;
  }
  return total;
}

function formatSyn(n: number): string {
  if (!Number.isFinite(n) || n === 0) return "0";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  if (n >= 1) return n.toFixed(2);
  return n.toFixed(4);
}

// Mirrors WalletPanel.formatBalance: thousands separators always; decimals
// dropped once the balance crosses 100k so the figure stays readable.
function formatSynBalance(v: number | null): string {
  if (v === null || !Number.isFinite(v)) return "—";
  return v.toLocaleString(undefined, {
    maximumFractionDigits: v >= 100_000 ? 0 : 4,
  });
}

export function MyNodePanel({
  password,
  chainInfo,
  status,
  onStart,
  onStop,
  onOpenLogs,
  onRefresh,
  startDisabled = false,
}: Props) {
  const [claiming, setClaiming] = useState(false);
  const [claimError, setClaimError] = useState<string | null>(null);
  const [claimTx, setClaimTx] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // Optimistic flag: flipped to `true` the instant a claim tx confirms.
  // The next chain-info poll will agree (vaultClaimableSyn → 0) and we
  // clear the flag. Until that happens we render zero locally so the user
  // can't double-click Claim while waiting for the RPC to catch up.
  const [recentlyClaimed, setRecentlyClaimed] = useState(false);

  const refresh = async () => {
    if (refreshing) return;
    setRefreshing(true);
    try {
      await onRefresh();
    } finally {
      setRefreshing(false);
    }
  };

  const handleClaim = async () => {
    if (!password) return;
    setClaiming(true);
    setClaimError(null);
    setClaimTx(null);
    try {
      const res = await invoke<CommandResult>("run_command", {
        command: "claim-wo-rewards",
        args: [],
        password,
      });
      if (!res.success) {
        setClaimError(res.error || res.output || "Claim failed");
        return;
      }
      // The CLI prints `__VAULT_CLAIM_OK__ <sig>` on confirmed success.
      // At this point the on-chain PDA is already zeroed — flip the
      // optimistic flag so the UI reflects the true state immediately
      // instead of waiting up to 15 s for the next chain-info poll.
      const match = res.output.match(/__VAULT_CLAIM_OK__\s+(\S+)/);
      if (match) setClaimTx(match[1]);
      setRecentlyClaimed(true);
      // Kick off a fresh read too, so the flag can self-clear quickly.
      void invoke("fetch_chain_info");
    } catch (e: unknown) {
      setClaimError(e instanceof Error ? e.message : String(e));
    } finally {
      setClaiming(false);
    }
  };

  // Clear the optimistic flag once the poll catches up — i.e. the RPC is
  // reporting vaultClaimableSyn === 0. Guarded so a fresh reward that
  // happens to arrive after a claim doesn't accidentally keep the UI zeroed.
  useEffect(() => {
    if (!recentlyClaimed) return;
    if ((chainInfo?.vaultClaimableSyn ?? 0) === 0) {
      setRecentlyClaimed(false);
    }
  }, [chainInfo?.vaultClaimableSyn, recentlyClaimed]);

  const wallet = chainInfo?.wallet ?? null;
  const nodeName = chainInfo?.nodeName ?? "Unknown Node";
  const tier = chainInfo?.tier ?? 0;
  const presence = chainInfo?.presencePoints ?? 0;
  const wins = chainInfo?.totalWins ?? 0;
  const submissions = chainInfo?.totalSubmissions ?? 0;
  // Zero it out locally until the poll confirms — stops accidental
  // double-claims during the RPC settlement window.
  const claimable = recentlyClaimed ? 0 : chainInfo?.vaultClaimableSyn ?? 0;
  const totalClaimed = chainInfo?.totalClaimedSyn ?? 0;
  const byType = recentlyClaimed ? {} : chainInfo?.rewardsByType ?? {};

  return (
    <div className="space-y-6">
      <PageHeader
        title="My Node"
        subtitle="Your node identity, rewards, and health"
        action={
          <>
            {chainInfo?.isBetaTester && <BetaBadge />}
            {status.running ? (
              <Button variant="danger" onClick={onStop}>
                <Square className="w-4 h-4" />
                Stop Node
              </Button>
            ) : (
              <Button
                variant="primary"
                onClick={onStart}
                disabled={!password || startDisabled}
              >
                <Play className="w-4 h-4" />
                {startDisabled ? "Locked" : "Start Node"}
              </Button>
            )}
            <Button variant="secondary" onClick={onOpenLogs}>
              <Terminal className="w-4 h-4" />
              Logs
            </Button>
            <Button variant="secondary" onClick={refresh} disabled={refreshing}>
              <RefreshCw className={`w-4 h-4 ${refreshing ? "animate-spin" : ""}`} />
              {refreshing ? "Refreshing…" : "Refresh"}
            </Button>
          </>
        }
      />

      {/* Identity card — node name, wallet, and current SYN balance share
          one row separated by a vertical divider. Stacks on small screens. */}
      <Card padding="md">
        <div className="flex items-center gap-4 flex-wrap md:flex-nowrap">
          <div className="w-12 h-12 rounded-xl bg-indigo-500/10 border border-indigo-500/20 flex items-center justify-center shrink-0">
            <Monitor className="w-6 h-6 text-indigo-400" />
          </div>
          <div className="flex-1 min-w-0">
            <div className="flex items-baseline gap-3 flex-wrap">
              <h2 className="text-lg font-bold text-slate-100 truncate">{nodeName}</h2>
              <span className="text-[10px] uppercase tracking-wider text-slate-500">
                Node Identity
              </span>
            </div>
            <p className="text-xs text-slate-400 font-mono break-all select-all mt-1">
              {wallet ?? "—"}
            </p>
          </div>
          <div className="hidden md:block w-px self-stretch bg-white/[0.06]" />
          <div className="text-right shrink-0 w-full md:w-auto pt-3 md:pt-0 border-t md:border-0 border-white/[0.06]">
            <p className="text-[10px] uppercase tracking-wider text-slate-500 mb-1">
              SYN Balance
            </p>
            <p
              className="text-2xl font-bold text-slate-100 font-mono leading-none"
              title={String(status.balance_syn ?? 0)}
            >
              {formatSynBalance(status.balance_syn ?? null)}
              <span className="ml-1 text-sm font-medium text-violet-400">SYN</span>
            </p>
          </div>
        </div>
      </Card>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <StatCard
          icon={<Star className="w-4 h-4 text-amber-400" />}
          label="STier"
          value={String(tier)}
          sub={["Free", "T1", "T2", "T3", "T4", "T5"][tier] ?? ""}
          valueClass="text-slate-100"
        />
        <StatCard
          icon={<Activity className="w-4 h-4 text-cyan-400" />}
          label="Presence"
          value={Math.round(presence).toLocaleString()}
          sub="points earned"
          valueClass="text-cyan-400"
        />
        <StatCard
          icon={<Trophy className="w-4 h-4 text-emerald-400" />}
          label="Wins"
          value={String(wins)}
          sub="rounds won"
          valueClass="text-emerald-400"
        />
        <StatCard
          icon={<Coins className="w-4 h-4 text-violet-400" />}
          label="Submissions"
          value={String(submissions)}
          sub="total submitted"
          valueClass="text-violet-400"
        />
      </div>

      {/* Rewards card */}
      <Card padding="md">
        <div className="flex items-center gap-3 mb-5">
          <div className="w-10 h-10 rounded-xl bg-emerald-500/10 flex items-center justify-center">
            <Coins className="w-5 h-5 text-emerald-400" />
          </div>
          <div>
            <h3 className="text-base font-semibold text-slate-100">Rewards</h3>
            <p className="text-xs text-slate-500">
              Earn SYN by running your node, winning rounds, and reviewing peers
            </p>
          </div>
        </div>

        {/* Claimable balance — sourced from the on-chain RewardAccount PDA,
            same as the web dashboard's Claim button. */}
        <div className="bg-emerald-500/5 border border-emerald-500/20 rounded-xl p-5 mb-5">
          <div className="flex items-center justify-between flex-wrap gap-4">
            <div>
              <p className="text-xs text-slate-500 mb-1">Claimable Balance (on-chain)</p>
              <p className="text-3xl font-bold text-emerald-400 font-mono">
                {claimable.toLocaleString(undefined, { maximumFractionDigits: 4 })}{" "}
                <span className="text-lg text-emerald-500">SYN</span>
              </p>
            </div>
            <button
              disabled={claiming || claimable === 0 || !password}
              onClick={handleClaim}
              className="px-6 py-3 rounded-xl bg-emerald-500 hover:bg-emerald-400 disabled:bg-slate-700 disabled:text-slate-500 text-white font-semibold transition-colors flex items-center gap-2"
            >
              {claiming && <Loader2 className="w-4 h-4 animate-spin" />}
              {claiming ? "Claiming…" : "Claim"}
            </button>
          </div>
          <p className="text-xs text-slate-600 mt-2">
            Total claimed: {formatSyn(totalClaimed)} SYN (lifetime) ·{" "}
            <button
              onClick={refresh}
              disabled={refreshing}
              className="underline hover:text-slate-400 disabled:opacity-50"
            >
              {refreshing ? "refreshing…" : "refresh"}
            </button>
          </p>
          {claimError && (
            <p className="text-xs text-red-400 mt-2">Claim failed: {claimError}</p>
          )}
          {claimTx && (
            <p className="text-xs text-emerald-400 mt-2 font-mono">
              Claimed. Tx: {claimTx.slice(0, 12)}…{claimTx.slice(-8)}
            </p>
          )}
        </div>

        {/* Breakdown by aggregated group. Hide groups whose sum is zero so the
            card stays focused on what the node actually earned. Unknown
            backend keys fall through to a gray fallback row. */}
        <div className="space-y-2">
          {(() => {
            const visibleGroups = REWARD_GROUPS
              .map((g) => ({ ...g, total: sumKeys(byType, g.keys) }))
              .filter((g) => g.total > 0)
              .sort((a, b) => a.order - b.order);

            const fallbackRows = Object.entries(byType)
              .filter(([key, amount]) => {
                if (KNOWN_REWARD_KEYS.has(key) || EXCLUDED_KEYS.has(key)) return false;
                const n = typeof amount === "number" ? amount : parseFloat(amount);
                return Number.isFinite(n) && n > 0;
              })
              .map(([key, amount]) => ({
                key,
                label: humanizeKey(key),
                color: "text-slate-400",
                total: typeof amount === "number" ? amount : parseFloat(amount) || 0,
              }));

            const hasAnyRow = visibleGroups.length > 0 || fallbackRows.length > 0;

            return (
              <>
                {visibleGroups.map((g) => (
                  <div
                    key={g.label}
                    className="flex items-center justify-between py-2 px-3 rounded bg-white/[0.02]"
                  >
                    <span className={`text-sm ${g.color}`}>{g.label}</span>
                    <span className="text-sm font-mono text-slate-100">
                      {formatSyn(g.total)} SYN
                    </span>
                  </div>
                ))}
                {fallbackRows.map((r) => (
                  <div
                    key={r.key}
                    className="flex items-center justify-between py-2 px-3 rounded bg-white/[0.02]"
                  >
                    <span className={`text-sm ${r.color}`}>{r.label}</span>
                    <span className="text-sm font-mono text-slate-100">
                      {formatSyn(r.total)} SYN
                    </span>
                  </div>
                ))}
                {!hasAnyRow && claimable === 0 && (
                  <p className="text-xs text-slate-600 text-center py-4">
                    No rewards yet — start running your node!
                  </p>
                )}
              </>
            );
          })()}
        </div>
      </Card>

      {/* Node Health */}
      <Card padding="md">
        <div className="flex items-center gap-3 mb-4">
          <div className="w-10 h-10 rounded-xl bg-violet-500/10 flex items-center justify-center">
            <Shield className="w-5 h-5 text-violet-400" />
          </div>
          <div>
            <h3 className="text-base font-semibold text-slate-100">Node Health</h3>
            <p className="text-xs text-slate-500">Anti-cheat status and integrity checks</p>
          </div>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
          <HealthItem
            label="Canary Tests"
            value={`${chainInfo?.canaryStrikes ?? 0}/3 strikes`}
            status={
              (chainInfo?.canaryStrikes ?? 0) === 0
                ? "good"
                : (chainInfo?.canaryStrikes ?? 0) >= 3
                  ? "bad"
                  : "warn"
            }
          />
          <HealthItem
            label="Anomaly Detection"
            value={`${chainInfo?.anomalyWarnings ?? 0}/3 warnings`}
            status={
              (chainInfo?.anomalyWarnings ?? 0) === 0
                ? "good"
                : (chainInfo?.anomalyWarnings ?? 0) >= 3
                  ? "bad"
                  : "warn"
            }
          />
          <HealthItem
            label="Binary Attestation"
            value={`${chainInfo?.attestationFailures ?? 0} failures`}
            status={
              (chainInfo?.attestationFailures ?? 0) === 0
                ? "good"
                : (chainInfo?.attestationFailures ?? 0) >= 5
                  ? "bad"
                  : "warn"
            }
          />
        </div>
      </Card>
    </div>
  );
}

function StatCard({
  icon,
  label,
  value,
  sub,
  valueClass,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  sub: string;
  valueClass: string;
}) {
  return (
    <Card padding="sm">
      <div className="flex items-center gap-2 mb-2">
        {icon}
        <span className="text-[10px] uppercase tracking-wider text-slate-500">{label}</span>
      </div>
      <div className={`text-3xl font-bold font-mono ${valueClass}`}>{value}</div>
      <span className="text-[10px] text-slate-600">{sub}</span>
    </Card>
  );
}

// Displayed only when the coord-side eligibility check flags the operator as a
// pre-mainnet beta tester. Purely informational — no claim flow lives here yet.
// Mirrors the dashboard `BetaBadge` 1:1 so web + desktop stay visually consistent.
function BetaBadge() {
  return (
    <span
      title="Joined before mainnet — eligible for the upcoming SYN airdrop snapshot."
      className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-emerald-500/10 border border-emerald-500/30 text-emerald-300 text-xs font-medium self-center"
    >
      <Sparkles className="w-3.5 h-3.5" aria-hidden="true" />
      Beta Tester
    </span>
  );
}

function HealthItem({
  label,
  value,
  status,
}: {
  label: string;
  value: string;
  status: "good" | "warn" | "bad";
}) {
  const Icon = status === "good" ? CheckCircle2 : status === "warn" ? AlertTriangle : XCircle;
  const color =
    status === "good" ? "text-emerald-400" : status === "warn" ? "text-amber-400" : "text-red-400";
  const bg =
    status === "good"
      ? "bg-emerald-500/5 border-emerald-500/20"
      : status === "warn"
        ? "bg-amber-500/5 border-amber-500/20"
        : "bg-red-500/5 border-red-500/20";
  return (
    <div className={`rounded-lg border p-3 ${bg}`}>
      <div className="flex items-center gap-2 mb-1">
        <Icon className={`w-4 h-4 ${color}`} />
        <span className="text-xs text-slate-400">{label}</span>
      </div>
      <p className={`text-sm font-medium ${color}`}>{value}</p>
    </div>
  );
}
