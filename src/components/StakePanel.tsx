import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChainInfo, CommandResult, NodeStatus } from "../App";
import { TrendingUp, TrendingDown, Gift, RefreshCw } from "lucide-react";
import { Card, Button, PageHeader, BalanceCard, Input } from "./ui";

interface Props {
  password: string | null;
  status: NodeStatus;
  chainInfo: ChainInfo | null;
  // Returns the freshly fetched ChainInfo so the post-stake poll can read
  // the on-chain value WITHOUT waiting for the `chainInfo` prop to update
  // (a prop doesn't change mid-async-function — see the poll loop below).
  onRefresh: () => Promise<ChainInfo | null>;
}

export function StakePanel({ password, status, chainInfo, onRefresh }: Props) {
  // One loading flag per action so clicking Claim doesn't freeze the Stake
  // and Unstake buttons with a shared "Processing…" label.
  const [stakingLoading, setStakingLoading] = useState(false);
  const [unstakingLoading, setUnstakingLoading] = useState(false);
  const [claimingLoading, setClaimingLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  // True while we're polling on-chain after the CLI returned success. Drives
  // the "Confirming on-chain…" label so the spinner stays up until the
  // Staked SYN card actually reflects the change (or we time out).
  const [confirming, setConfirming] = useState(false);

  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Soft, non-error notice (e.g. "submitted but RPC hasn't caught up yet").
  // Rendered in a neutral style — NOT the red error card — because the tx
  // most likely landed and only the read lagged.
  const [notice, setNotice] = useState<string | null>(null);
  const [stakeAmount, setStakeAmount] = useState("0");
  const [unstakeAmount, setUnstakeAmount] = useState("0");

  // Unmount guard: the poll loop awaits across several seconds, so we must
  // not call setState after the panel is gone (user switched tabs mid-poll).
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const stakedMax = chainInfo?.staked ?? 0;
  const availableMax = status.balance_syn ?? 0;
  const stakeNum = Number.parseFloat(stakeAmount) || 0;
  const unstakeNum = Number.parseFloat(unstakeAmount) || 0;
  const stakeExceedsAvailable = stakeNum > availableMax;
  const unstakeExceedsStaked = unstakeNum > stakedMax;

  // Promise-based sleep (no deps). Used to space out the confirmation poll.
  const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

  // On-chain confirmation poll. Spaced ~3 s apart, capped at ~45 s
  // (POLL_ATTEMPTS × POLL_INTERVAL_MS). Reads the FRESH staked value from
  // the value `onRefresh()` RETURNS — the `chainInfo` prop won't update
  // inside this async function, so relying on it would loop forever on the
  // stale closure value.
  const POLL_INTERVAL_MS = 3000;
  const POLL_ATTEMPTS = 15;

  const runCmd = async (
    cmd: string,
    args: string[],
    action: string,
    setLoading: (v: boolean) => void,
  ) => {
    if (!password) return;
    setLoading(true);
    setError(null);
    setResult(null);
    setNotice(null);
    try {
      const res = await invoke<CommandResult>("run_command", { command: cmd, args, password });
      if (res.success) {
        setResult(res.output.trim() || `${action} completed successfully`);
      } else {
        setError(res.error || res.output || `${action} failed`);
      }
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  // Stake/unstake with on-chain confirmation. The CLI returning success only
  // means the tx was submitted — the Staked SYN card stays stale until the
  // RPC read reflects it. So after success we keep the spinner up, poll the
  // chain, and only clear loading once the new value lands (or we time out).
  const runStakingCmd = async (
    cmd: "stake" | "unstake",
    amount: number,
    amountStr: string,
    action: string,
    setLoading: (v: boolean) => void,
  ) => {
    if (!password) return;
    // Capture the BEFORE value so we know what "confirmed" looks like.
    // Track whether we actually had a baseline: if chainInfo was null at click
    // (RPC down / first load), a `?? 0` baseline would make any pre-existing
    // stake look like an instant "confirmed" — so we fail closed below instead.
    const baselineKnown = chainInfo?.staked != null;
    const prevStaked = chainInfo?.staked ?? 0;
    setLoading(true);
    setError(null);
    setResult(null);
    setNotice(null);
    try {
      const res = await invoke<CommandResult>("run_command", {
        command: cmd,
        args: [amountStr],
        password,
      });
      if (!res.success) {
        // CLI rejected the tx — surface the error and stop, no polling.
        setError(res.error || res.output || `${action} failed`);
        return;
      }
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
      return;
    } finally {
      // The CLI call is done; clear the per-action flag. From here on the
      // shared `confirming` flag drives the button label.
      if (mountedRef.current) setLoading(false);
    }

    // CLI succeeded. If we never had a prior on-chain baseline, we cannot
    // honestly confirm a delta — fail closed to a soft notice rather than risk
    // a false "confirmed" against a pre-existing balance.
    if (!baselineKnown) {
      if (mountedRef.current) {
        setNotice("Submitted on-chain; couldn't read prior balance to confirm — hit Refresh in a moment.");
      }
      return;
    }

    // CLI succeeded — poll on-chain until the change is reflected. 0.99
    // tolerance absorbs network fees / decimal rounding. Assumes a single
    // actor: a concurrent stake/unstake from elsewhere can move `staked`
    // against the expected direction and force the timeout notice.
    const tolerance = amount * 0.99;
    if (mountedRef.current) setConfirming(true);
    try {
      for (let attempt = 0; attempt < POLL_ATTEMPTS; attempt++) {
        await sleep(POLL_INTERVAL_MS);
        if (!mountedRef.current) return;
        const fresh = await onRefresh();
        const freshStaked = fresh?.staked ?? prevStaked;
        const confirmed =
          cmd === "stake"
            ? freshStaked >= prevStaked + tolerance
            : freshStaked <= prevStaked - tolerance;
        if (confirmed) {
          if (mountedRef.current) {
            setResult(`${action} confirmed — staked is now ${fmt(freshStaked)} SYN`);
          }
          return;
        }
      }
      // Timed out waiting for the read to catch up. The tx very likely
      // landed; this is a soft notice, not a failure.
      if (mountedRef.current) {
        setNotice("Submitted on-chain; balance not reflected yet — hit Refresh in a moment.");
      }
    } finally {
      if (mountedRef.current) setConfirming(false);
    }
  };

  const handleStake = () => {
    if (!stakeNum) {
      setError("Enter an amount to stake");
      return;
    }
    if (stakeExceedsAvailable) {
      setError(`Cannot stake more than ${availableMax.toLocaleString()} SYN (available balance)`);
      return;
    }
    runStakingCmd("stake", stakeNum, stakeAmount, "Stake", setStakingLoading);
  };

  const handleUnstake = () => {
    if (!unstakeNum) {
      setError("Enter an amount to unstake");
      return;
    }
    if (unstakeExceedsStaked) {
      setError(`Cannot unstake more than ${stakedMax.toLocaleString()} SYN`);
      return;
    }
    runStakingCmd("unstake", unstakeNum, unstakeAmount, "Unstake", setUnstakingLoading);
  };

  const handleClaimRewards = () =>
    runCmd("claim-rewards", [], "Claim rewards", setClaimingLoading);

  // Real refresh: drives the shared App-level chain-info fetch so the
  // cards on this page actually update. Used to be a no-op invoke that
  // hit Rust but never propagated back to this component's props.
  const handleRefresh = async () => {
    if (refreshing) return;
    setRefreshing(true);
    try {
      await onRefresh();
    } finally {
      setRefreshing(false);
    }
  };

  const handleMaxUnstake = () => {
    if (stakedMax > 0) setUnstakeAmount(stakedMax.toString());
  };
  const handleMaxStake = () => {
    if (availableMax > 0) setStakeAmount(availableMax.toString());
  };

  // Default formatter keeps up to 4 decimals — readable for small balances.
  const fmt = (v: number | null) =>
    v === null ? "—" : v.toLocaleString(undefined, { maximumFractionDigits: 4 });

  // Large-balance formatter: drop decimals once we're past 100k, otherwise
  // the digit chain gets noisy in a card tile. Applied only to Available SYN.
  const fmtCompact = (v: number | null) => {
    if (v === null) return "—";
    if (v >= 100_000) return Math.round(v).toLocaleString();
    return v.toLocaleString(undefined, { maximumFractionDigits: 4 });
  };

  // Factory: clamp user-typed value to [0, max]. Keeps slider + input in
  // sync without fighting the user mid-type — empty/partial strings stay so
  // they can finish typing decimals before we lock the value.
  const makeClampedChange =
    (setter: (v: string) => void, max: number) =>
    (raw: string) => {
      if (raw === "" || raw === ".") {
        setter(raw);
        return;
      }
      const parsed = Number.parseFloat(raw);
      if (Number.isNaN(parsed)) return;
      if (parsed < 0) {
        setter("0");
        return;
      }
      if (parsed > max) {
        setter(max.toString());
        return;
      }
      setter(raw);
    };
  const handleStakeChange = makeClampedChange(setStakeAmount, availableMax);
  const handleUnstakeChange = makeClampedChange(setUnstakeAmount, stakedMax);

  // Dark, token-matched slider. Tailwind `accent-*` only styles the thumb on
  // most browsers — the track ends up in the browser default (bright blue on
  // macOS). The inline class blends both into the UI: low-contrast slate
  // track with a cyan thumb.
  const sliderClass =
    "w-full h-2 appearance-none rounded-full " +
    "bg-[var(--bg-elevated)] accent-slate-500 " +
    "disabled:opacity-40 cursor-pointer";

  return (
    <div className="space-y-6">
      <PageHeader
        title="Stake"
        subtitle="Lock SYN to earn rewards and unlock higher tiers"
        action={
          <>
            <Button
              variant="secondary"
              onClick={handleRefresh}
              disabled={refreshing}
            >
              <RefreshCw className={`w-4 h-4 ${refreshing ? "animate-spin" : ""}`} />
              {refreshing ? "Refreshing…" : "Refresh"}
            </Button>
          </>
        }
      />

      {/* Overview — on-chain StakeAccount reads */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <BalanceCard
          label="Staked SYN"
          value={fmt(chainInfo?.staked ?? status.staked_syn)}
          symbol="SYN"
          accent="green"
        />
        <BalanceCard
          label="Pending Rewards"
          value={fmt(chainInfo?.rewardsPending ?? null)}
          symbol="SYN"
          accent="amber"
          hint="Claimable now"
        />
        <BalanceCard
          label="Current Tier"
          value={status.tier !== null ? `Tier ${status.tier}` : "—"}
          accent="cyan"
        />
        <BalanceCard
          label="Available SYN"
          value={fmtCompact(status.balance_syn)}
          symbol="SYN"
          accent="purple"
        />
      </div>

      {/* Stake / Unstake */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <Card padding="md">
          <div className="flex items-center gap-2 mb-4">
            <TrendingUp className="w-5 h-5 text-emerald-400" />
            <h2 className="text-lg font-semibold text-slate-100">Stake SYN</h2>
          </div>
          <p className="text-sm text-slate-400 mb-4">
            Lock your SYN to earn rewards and unlock higher tiers.
          </p>

          <div className="space-y-3">
            <div className="flex items-center justify-between text-xs text-slate-400">
              <span>
                Available: <span className="text-slate-200 font-mono">{fmtCompact(availableMax)}</span> SYN
              </span>
              <button
                type="button"
                onClick={handleMaxStake}
                disabled={availableMax <= 0}
                className="text-[var(--accent-cyan)] hover:opacity-80 disabled:opacity-40 disabled:cursor-not-allowed transition-opacity"
              >
                Max
              </button>
            </div>
            <input
              type="range"
              min={0}
              max={availableMax > 0 ? availableMax : 1}
              step={Math.max(availableMax / 1000, 0.01)}
              value={stakeNum}
              onChange={(e) => setStakeAmount(e.target.value)}
              disabled={availableMax <= 0}
              className={sliderClass}
            />
            <Input
              label="Amount (SYN)"
              type="number"
              value={stakeAmount}
              onChange={(e) => handleStakeChange(e.target.value)}
              placeholder="0.0"
              error={stakeExceedsAvailable ? "Exceeds available balance" : undefined}
            />
          </div>

          <Button
            className="mt-4 w-full"
            variant="primary"
            size="lg"
            onClick={handleStake}
            disabled={
              stakingLoading ||
              confirming ||
              !password ||
              stakeNum <= 0 ||
              stakeExceedsAvailable
            }
          >
            {stakingLoading ? "Processing…" : confirming ? "Confirming on-chain…" : "Stake"}
          </Button>
        </Card>

        <Card padding="md">
          <div className="flex items-center gap-2 mb-4">
            <TrendingDown className="w-5 h-5 text-amber-400" />
            <h2 className="text-lg font-semibold text-slate-100">Unstake SYN</h2>
          </div>
          <p className="text-sm text-slate-400 mb-4">
            Release staked SYN. Capped at your current stake.
          </p>

          {/* Slider + number input stay in sync. Slider max = stakedMax so
              you literally can't push it past your balance. The text input
              clamps on edit to the same bound. */}
          <div className="space-y-3">
            <div className="flex items-center justify-between text-xs text-slate-400">
              <span>
                Staked: <span className="text-slate-200 font-mono">{fmt(stakedMax)}</span> SYN
              </span>
              <button
                type="button"
                onClick={handleMaxUnstake}
                disabled={stakedMax <= 0}
                className="text-[var(--accent-cyan)] hover:opacity-80 disabled:opacity-40 disabled:cursor-not-allowed transition-opacity"
              >
                Max
              </button>
            </div>
            <input
              type="range"
              min={0}
              max={stakedMax > 0 ? stakedMax : 1}
              step={Math.max(stakedMax / 1000, 0.01)}
              value={unstakeNum}
              onChange={(e) => setUnstakeAmount(e.target.value)}
              disabled={stakedMax <= 0}
              className={sliderClass}
            />
            <Input
              label="Amount (SYN)"
              type="number"
              value={unstakeAmount}
              onChange={(e) => handleUnstakeChange(e.target.value)}
              placeholder="0.0"
              error={unstakeExceedsStaked ? "Exceeds staked balance" : undefined}
            />
          </div>

          <Button
            className="mt-4 w-full"
            variant="danger"
            size="lg"
            onClick={handleUnstake}
            disabled={
              unstakingLoading ||
              confirming ||
              !password ||
              unstakeNum <= 0 ||
              unstakeExceedsStaked
            }
          >
            {unstakingLoading ? "Processing…" : confirming ? "Confirming on-chain…" : "Unstake"}
          </Button>
        </Card>
      </div>

      {/* Claim staking rewards */}
      <Card padding="md">
        <div className="flex items-center justify-between gap-4 flex-wrap">
          <div className="flex items-center gap-3">
            <Gift className="w-6 h-6 text-purple-400" />
            <div>
              <h2 className="text-lg font-semibold text-slate-100">Claim Staking Rewards</h2>
              <p className="text-sm text-slate-400">
                Sweep {fmt(chainInfo?.rewardsPending ?? null)} SYN of pending staking rewards to your wallet.
              </p>
            </div>
          </div>
          <Button
            variant="primary"
            size="lg"
            onClick={handleClaimRewards}
            disabled={claimingLoading || confirming || !password || !chainInfo?.rewardsPending}
          >
            {claimingLoading ? "Processing…" : "Claim"}
          </Button>
        </div>
      </Card>

      {/* Soft, non-error notice (timed-out confirmation poll). Neutral
          cyan style — distinct from both the green success and red error
          cards — so a slow RPC read doesn't read as a failed transaction. */}
      {notice && !error && (
        <Card padding="sm" className="border-cyan-500/30 bg-cyan-500/10">
          <p className="text-sm text-cyan-200 whitespace-pre-wrap break-words">{notice}</p>
        </Card>
      )}

      {(result || error) && (
        <Card
          padding="sm"
          className={error ? "border-red-500/30 bg-red-500/10" : "border-emerald-500/30 bg-emerald-500/10"}
        >
          {error ? (
            <p className="text-sm text-red-300 whitespace-pre-wrap break-words">{error}</p>
          ) : (
            <pre className="text-sm text-emerald-300 whitespace-pre-wrap overflow-x-auto font-mono">
              {result}
            </pre>
          )}
        </Card>
      )}
    </div>
  );
}
