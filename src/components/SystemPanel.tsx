import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Server, Cpu, MemoryStick, Monitor, RefreshCw, Sparkles } from "lucide-react";
import { Card, Button, PageHeader } from "./ui";

interface SystemInfo {
  os: string;
  cpuModel: string;
  cpuCores: number;
  ramGb: number;
  gpuType: string | null;
  gpuVramGb: number | null;
  recommendedTier: number;
  hasOllama: boolean;
}

const tierName = (tier: number): string => {
  const names = [
    "CPU-Only",
    "Tier 1 · 8GB GPU",
    "Tier 2 · 16GB GPU",
    "Tier 3 · 24GB GPU",
    "Tier 4 · 32GB GPU",
    "Tier 5 · 80GB GPU",
  ];
  return names[tier] ?? `Tier ${tier}`;
};

export function SystemPanel() {
  const [loading, setLoading] = useState(false);
  const [info, setInfo] = useState<SystemInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    if (loading) return;
    setLoading(true);
    setError(null);
    try {
      const res = await invoke<SystemInfo>("system_info");
      setInfo(res);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const fmtGb = (n: number | null | undefined) =>
    n === null || n === undefined ? "—" : `${n.toLocaleString(undefined, { maximumFractionDigits: 1 })} GB`;

  return (
    <div className="space-y-6">
      <PageHeader
        title="System"
        subtitle="Hardware capabilities and tier assignment"
        action={
          <Button variant="secondary" onClick={refresh} disabled={loading}>
            <RefreshCw className={`w-4 h-4 ${loading ? "animate-spin" : ""}`} />
            {loading ? "Scanning…" : "Refresh"}
          </Button>
        }
      />

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <SystemCard icon={Monitor} accent="text-cyan-400" label="Operating System" value={info?.os ?? null} />
        <SystemCard
          icon={Cpu}
          accent="text-emerald-400"
          label="CPU"
          value={info?.cpuModel ?? null}
          sub={info?.cpuCores ? `${info.cpuCores} cores` : null}
        />
        <SystemCard
          icon={MemoryStick}
          accent="text-purple-400"
          label="Memory"
          value={info ? fmtGb(info.ramGb) : null}
        />
        <SystemCard
          icon={Server}
          accent="text-amber-400"
          label="GPU"
          value={info ? info.gpuType ?? "None detected" : null}
          sub={info && info.gpuVramGb ? `${fmtGb(info.gpuVramGb)} VRAM` : null}
        />
      </div>

      <Card padding="md">
        <h2 className="text-lg font-semibold text-slate-100 mb-4">Hardware Tier</h2>
        <div className="flex items-start gap-4 flex-wrap">
          <Card padding="sm" className="border-cyan-500/30 bg-cyan-500/10">
            <p className="text-xs uppercase tracking-wide text-slate-400 mb-1">Recommended</p>
            <p className="text-2xl font-bold text-[var(--accent-cyan)]">
              {info ? tierName(info.recommendedTier) : "—"}
            </p>
          </Card>
          <Card
            padding="sm"
            className={info?.hasOllama ? "border-emerald-500/30 bg-emerald-500/10" : "border-white/[0.06]"}
          >
            <div className="flex items-center gap-2 mb-1">
              <Sparkles className={`w-4 h-4 ${info?.hasOllama ? "text-emerald-400" : "text-slate-400"}`} />
              <p className="text-xs uppercase tracking-wide text-slate-400">Local Inference (Ollama)</p>
            </div>
            <p
              className={`text-2xl font-bold ${
                info?.hasOllama ? "text-emerald-400" : "text-slate-400"
              }`}
            >
              {info === null ? "—" : info.hasOllama ? "Available" : "Not detected"}
            </p>
          </Card>
        </div>
        <p className="text-sm text-slate-400 mt-4 max-w-2xl">
          Your hardware tier determines the work orders your node can handle and the rewards you earn.
          Higher tiers unlock heavier inference tasks.
        </p>
      </Card>

      {error && (
        <Card padding="sm" className="border-red-500/30 bg-red-500/10">
          <p className="text-sm text-red-300 break-words">{error}</p>
        </Card>
      )}
    </div>
  );
}

function SystemCard({
  icon: Icon,
  accent,
  label,
  value,
  sub,
}: {
  icon: typeof Monitor;
  accent: string;
  label: string;
  value: string | null | undefined;
  sub?: string | null;
}) {
  return (
    <Card padding="md">
      <div className="flex items-center gap-3 mb-4">
        <Icon className={`w-6 h-6 ${accent}`} />
        <h2 className="text-sm uppercase tracking-wide text-slate-400 font-medium">{label}</h2>
      </div>
      <p className="text-xl font-bold text-slate-100 break-words">{value ?? "—"}</p>
      {sub && <p className="text-sm text-slate-400 mt-1">{sub}</p>}
    </Card>
  );
}
