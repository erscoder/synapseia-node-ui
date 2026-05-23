import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChainInfo, CommandResult, NodeStatus } from "../App";
import { Wallet, ArrowUpCircle, Eye, EyeOff, Copy, Check, RefreshCw, AlertTriangle } from "lucide-react";
import { Card, Button, PageHeader, BalanceCard, Input } from "./ui";

interface Props {
  password: string | null;
  status: NodeStatus;
  chainInfo: ChainInfo | null;
  // Returns the freshly fetched ChainInfo (shared with StakePanel's
  // confirmation poll). This panel awaits it and ignores the value.
  onRefresh: () => Promise<ChainInfo | null>;
}

export function WalletPanel({ password, status, chainInfo, onRefresh }: Props) {
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [destination, setDestination] = useState("");
  const [amount, setAmount] = useState("");
  const [asset, setAsset] = useState<"sol" | "syn">("sol");
  const [showKey, setShowKey] = useState(false);
  const [exportedKey, setExportedKey] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const handleRefresh = async () => {
    if (refreshing) return;
    setRefreshing(true);
    try {
      await onRefresh();
    } finally {
      setRefreshing(false);
    }
  };

  const handleCopyAddress = async () => {
    if (!status.wallet) return;
    await navigator.clipboard.writeText(status.wallet);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleWithdraw = async () => {
    if (!password || !destination) {
      setError("Destination address required");
      return;
    }
    setLoading(true);
    setError(null);
    setSuccess(null);
    try {
      const command = asset === "sol" ? "withdraw-sol" : "withdraw-syn";
      const args = amount ? [amount, destination] : [destination];
      const res = await invoke<CommandResult>("run_command", {
        command,
        args,
        password,
      });
      if (!res.success) {
        setError(res.error || res.output || "Withdrawal failed");
      } else {
        setSuccess("Withdrawal submitted. It may take a few seconds to confirm.");
        setDestination("");
        setAmount("");
        void handleRefresh();
      }
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  const handleExportKey = async () => {
    if (!password) return;
    setLoading(true);
    setError(null);
    try {
      const res = await invoke<CommandResult>("run_command", {
        command: "export-key",
        args: [],
        password,
      });
      // The CLI prints the base58 key wrapped in a lot of banner lines and
      // ANSI colour codes. Pull the value out of the `__PRIVATE_KEY__`
      // sentinel — every other attempt at parsing the formatted output is
      // fragile.
      const match = res.output.match(/__PRIVATE_KEY__\s+([1-9A-HJ-NP-Za-km-z]+)/);
      if (res.success && match) {
        setExportedKey(match[1]);
        setShowKey(true);
      } else if (!res.success) {
        setError(res.error || res.output || "Failed to export key");
      } else {
        setError(
          "Could not locate the private key in the CLI output. " +
            "Make sure the node CLI is the latest build.",
        );
      }
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  // Drop decimals once the balance crosses 100k — keeps large figures
  // readable while preserving precision on small ones.
  const formatBalance = (v: number | null) =>
    v === null
      ? "—"
      : v.toLocaleString(undefined, { maximumFractionDigits: v >= 100_000 ? 0 : 4 });

  return (
    <div className="space-y-6">
      <PageHeader
        title="Wallet"
        subtitle="Solana Devnet"
        action={
          <Button variant="secondary" onClick={handleRefresh} disabled={refreshing}>
            <RefreshCw className={`w-4 h-4 ${refreshing ? "animate-spin" : ""}`} />
            {refreshing ? "Refreshing…" : "Refresh"}
          </Button>
        }
      />

      {/* Address card — mirrors the Identity card layout on My Node. Icon
          tile on the left, label + monospaced address in the middle, Copy
          button on the right (replaces the old page-level Refresh since
          Refresh already lives in the page header). */}
      <Card padding="md">
        <div className="flex items-center gap-4 flex-wrap">
          <div className="w-12 h-12 rounded-xl bg-cyan-500/10 border border-cyan-500/20 flex items-center justify-center shrink-0">
            <Wallet className="w-6 h-6 text-[var(--accent-cyan)]" />
          </div>
          <div className="flex-1 min-w-0">
            <div className="flex items-baseline gap-3 flex-wrap">
              <h2 className="text-lg font-bold text-slate-100">Wallet</h2>
              <span className="text-[10px] uppercase tracking-wider text-slate-500">
                Address
              </span>
            </div>
            <p className="text-xs text-slate-400 font-mono break-all select-all mt-1">
              {status.wallet ?? "—"}
            </p>
          </div>
          <Button
            variant="primary"
            size="lg"
            onClick={handleCopyAddress}
            disabled={!status.wallet}
          >
            {copied ? <Check className="w-4 h-4" /> : <Copy className="w-4 h-4" />}
            {copied ? "Copied" : "Copy"}
          </Button>
        </div>
      </Card>

      {/* Balances — stake + pending rewards read direct from chain */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <BalanceCard label="SOL Balance" value={formatBalance(status.balance_sol)} symbol="SOL" accent="blue" />
        <BalanceCard label="SYN Balance" value={formatBalance(status.balance_syn)} symbol="SYN" accent="purple" />
        <BalanceCard
          label="Staked SYN"
          value={formatBalance(chainInfo?.staked ?? status.staked_syn)}
          symbol="SYN"
          accent="green"
        />
        <BalanceCard
          label="Pending Rewards"
          value={formatBalance(chainInfo?.rewardsPending ?? null)}
          symbol="SYN"
          accent="amber"
        />
      </div>

      {/* Withdraw — pick asset, drag slider or type, hit Withdraw. */}
      {(() => {
        const sliderClass =
          "w-full h-2 appearance-none rounded-full " +
          "bg-[var(--bg-elevated)] accent-slate-500 " +
          "disabled:opacity-40 cursor-pointer";
        const availableMax =
          asset === "sol" ? status.balance_sol ?? 0 : status.balance_syn ?? 0;
        const symbol = asset === "sol" ? "SOL" : "SYN";
        const accentClass = asset === "sol" ? "text-blue-400" : "text-violet-400";
        const amountNum = Number.parseFloat(amount) || 0;
        const exceedsMax = amountNum > availableMax;
        const setAmountClamped = (raw: string) => {
          if (raw === "") return setAmount("");
          const n = Number.parseFloat(raw);
          if (!Number.isFinite(n) || n < 0) return;
          setAmount(String(Math.min(n, availableMax)));
        };
        return (
          <Card padding="md">
            <div className="flex items-center gap-2 mb-4">
              <ArrowUpCircle className="w-5 h-5 text-orange-400" />
              <h2 className="text-lg font-semibold text-slate-100">Withdraw</h2>
            </div>

            {/* Asset toggle */}
            <div className="inline-flex p-1 rounded-xl bg-[var(--bg-elevated)] border border-white/[0.06] mb-5">
              {(["sol", "syn"] as const).map((a) => {
                const active = asset === a;
                const activeColor =
                  a === "sol" ? "bg-blue-500/15 text-blue-300" : "bg-violet-500/15 text-violet-300";
                return (
                  <button
                    key={a}
                    type="button"
                    onClick={() => {
                      setAsset(a);
                      setAmount("");
                    }}
                    className={`px-4 py-1.5 rounded-lg text-sm font-medium transition-colors ${
                      active ? activeColor : "text-slate-400 hover:text-slate-200"
                    }`}
                  >
                    {a.toUpperCase()}
                  </button>
                );
              })}
            </div>

            <Input
              label="Destination address"
              value={destination}
              onChange={(e) => setDestination(e.target.value)}
              placeholder="Solana address"
            />

            <div className="mt-5 space-y-3">
              <div className="flex items-center justify-between text-xs">
                <span className="text-slate-400">
                  Amount ({symbol})
                </span>
                <span className="text-slate-500">
                  Available:{" "}
                  <span className="text-slate-300 font-mono">
                    {formatBalance(availableMax)}
                  </span>{" "}
                  <span className={accentClass}>{symbol}</span>
                </span>
              </div>
              <input
                type="range"
                min={0}
                max={availableMax > 0 ? availableMax : 1}
                step={Math.max(availableMax / 1000, 0.0001)}
                value={amountNum}
                onChange={(e) => setAmount(e.target.value)}
                disabled={availableMax <= 0}
                className={sliderClass}
              />
              <div className="flex items-stretch gap-2">
                <div className="flex-1">
                  <Input
                    type="number"
                    value={amount}
                    onChange={(e) => setAmountClamped(e.target.value)}
                    placeholder="0.0"
                    error={exceedsMax ? `Exceeds available ${symbol}` : undefined}
                  />
                </div>
                <Button
                  variant="secondary"
                  onClick={() => setAmount(String(availableMax))}
                  disabled={availableMax <= 0}
                >
                  Max
                </Button>
              </div>
            </div>

            <Button
              variant="danger"
              onClick={handleWithdraw}
              disabled={
                loading ||
                !password ||
                !destination ||
                exceedsMax ||
                amountNum <= 0
              }
              className="mt-5"
            >
              {loading ? "Processing…" : `Withdraw ${symbol}`}
            </Button>
          </Card>
        );
      })()}

      {/* Export Key */}
      <Card padding="md">
        <div className="flex items-center gap-2 mb-4">
          <Eye className="w-5 h-5 text-red-400" />
          <h2 className="text-lg font-semibold text-slate-100">Export Private Key</h2>
        </div>
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-3 flex items-start gap-2 mb-4">
          <AlertTriangle className="w-4 h-4 text-red-400 mt-0.5 flex-shrink-0" />
          <p className="text-sm text-red-300">
            Anyone with this key can move your funds. Never paste it into a website.
          </p>
        </div>
        <Button variant="danger" onClick={handleExportKey} disabled={loading || !password}>
          {loading ? "Exporting…" : "Show private key"}
        </Button>

        {showKey && exportedKey && (
          <div className="flex items-center gap-2 mt-4">
            <code className="flex-1 bg-[var(--bg-elevated)]/80 border border-white/[0.06] px-4 py-2 rounded-lg text-sm text-slate-300 break-all font-mono">
              {showKey ? exportedKey : "••••••••••••"}
            </code>
            <Button variant="ghost" size="md" onClick={() => setShowKey(!showKey)}>
              {showKey ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
            </Button>
            <Button
              variant="ghost"
              size="md"
              onClick={async () => {
                await navigator.clipboard.writeText(exportedKey);
                setCopied(true);
                setTimeout(() => setCopied(false), 2000);
              }}
            >
              {copied ? <Check className="w-4 h-4 text-emerald-400" /> : <Copy className="w-4 h-4" />}
            </Button>
          </div>
        )}
      </Card>

      {error && (
        <Card padding="sm" className="border-red-500/30 bg-red-500/10">
          <p className="text-sm text-red-300 break-words">{error}</p>
        </Card>
      )}
      {success && (
        <Card padding="sm" className="border-emerald-500/30 bg-emerald-500/10">
          <p className="text-sm text-emerald-300 break-words">{success}</p>
        </Card>
      )}
    </div>
  );
}
