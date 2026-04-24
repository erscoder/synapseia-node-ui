import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Copy, Check } from "lucide-react";
import { ChainInfo } from "../App";
import { Card, Button } from "./ui";

interface Props {
  walletAddress: string;
  onActivated: () => void;
  password: string;
}

export function ActivationScreen({ walletAddress, onActivated }: Props) {
  const [status, setStatus] = useState<"waiting" | "deposited">("waiting");
  const [copied, setCopied] = useState(false);

  const checkActivation = async () => {
    try {
      const info = await invoke<ChainInfo>("fetch_chain_info");
      // Activated once we see SOL arrive. If the token account also exists,
      // great — but landing ≥ rent-exempt SOL is enough proof for the UI to
      // advance (the node runtime will create the token account on first
      // start).
      if (info.sol > 0.001) {
        setStatus("deposited");
        setTimeout(() => onActivated(), 1200);
      }
    } catch (e) {
      console.warn("Activation check failed:", e);
    }
  };

  const handleCopyAddress = async () => {
    try {
      await navigator.clipboard.writeText(walletAddress);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (e) {
      console.error("Failed to copy:", e);
    }
  };

  useEffect(() => {
    const interval = setInterval(checkActivation, 10000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="flex flex-col items-center justify-center min-h-screen text-slate-100 py-10">
      <div className="w-full max-w-md p-8 space-y-6">
        <div className="text-center space-y-2">
          <div className="w-20 h-20 mx-auto flex items-center justify-center">
            <img
              src="/synapseia-logo.png"
              alt="Synapseia"
              className="w-full h-full object-contain drop-shadow-[0_0_40px_rgba(0,212,255,0.25)]"
            />
          </div>
          <h1 className="text-3xl font-bold text-slate-100 tracking-tight">Activate Your Node</h1>
          <p className="text-slate-400 text-sm">
            {status === "waiting" && "Send SOL to activate your wallet"}
            {status === "deposited" && "Deposit detected — activating…"}
          </p>
        </div>

        <Card padding="md" className="space-y-4">
          <div className="space-y-1.5">
            <label className="block text-xs uppercase tracking-wide text-slate-400">Your Wallet Address</label>
            <div className="flex gap-2">
              <input
                type="text"
                readOnly
                value={walletAddress}
                className="flex-1 px-4 py-3 bg-[var(--bg-elevated)]/80 border border-white/[0.06] rounded-lg text-slate-100 text-sm font-mono"
              />
              <Button variant="primary" size="lg" onClick={handleCopyAddress}>
                {copied ? <Check className="w-4 h-4" /> : <Copy className="w-4 h-4" />}
                {copied ? "Copied" : "Copy"}
              </Button>
            </div>
          </div>

          <div className="bg-cyan-500/10 border border-cyan-500/30 rounded-lg p-4">
            <p className="text-cyan-300 text-sm font-medium mb-1">Send 0.05 SOL to activate</p>
            <p className="text-slate-400 text-xs">
              This deposit creates your SYN token account (rent exemption) so your node
              can receive network rewards.
            </p>
          </div>

          {status === "waiting" ? (
            <div className="flex items-center justify-center gap-3 py-4">
              <div className="w-4 h-4 border-2 border-cyan-400 border-t-transparent rounded-full animate-spin" />
              <span className="text-slate-400 text-sm">Waiting for deposit…</span>
            </div>
          ) : (
            <div className="flex items-center justify-center gap-3 py-4">
              <div className="w-2 h-2 bg-emerald-400 rounded-full animate-pulse" />
              <span className="text-emerald-400 text-sm">Deposit detected</span>
            </div>
          )}
        </Card>

        <p className="text-xs text-slate-500 text-center">
          You can send SOL from an exchange or another wallet.
        </p>
      </div>
    </div>
  );
}
