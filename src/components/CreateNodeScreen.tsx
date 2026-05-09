import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input } from "./ui";
import {
  CLOUD_PROVIDERS_UI,
  OLLAMA_DEFAULT_MODELS,
  buildSlug,
  type CloudProviderId,
  type ModelTier,
} from "../lib/providers";

interface Props {
  onCreated: (password: string, walletAddress: string) => void;
}

interface CreateResult {
  success: boolean;
  wallet_address: string | null;
  error_code: string | null;
  error_message: string | null;
}

type ProviderSelection =
  | { kind: "cloud"; provider: CloudProviderId; tier: ModelTier }
  | { kind: "ollama"; modelId: string };

const DEFAULT_SELECTION: ProviderSelection = {
  kind: "cloud",
  provider: "anthropic",
  tier: "mid",
};

function slugFromSelection(sel: ProviderSelection): string {
  if (sel.kind === "ollama") return buildSlug("ollama", sel.modelId);
  const entry = CLOUD_PROVIDERS_UI.find(p => p.id === sel.provider);
  if (!entry) return "anthropic/claude-sonnet-4-6";
  return buildSlug(entry.id, entry.models[sel.tier].modelId);
}

export function CreateNodeScreen({ onCreated }: Props) {
  const [nodeName, setNodeName] = useState("");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [selection, setSelection] = useState<ProviderSelection>(DEFAULT_SELECTION);
  const [llmKey, setLlmKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const isLocalModel = selection.kind === "ollama";
  // Ollama needs no key; cloud always needs one to do anything useful.
  const needsCredentials = !isLocalModel;

  const handleCreate = async () => {
    setError(null);

    if (!nodeName.trim()) return setError("Node name is required.");
    if (password.length < 8) return setError("Password must be at least 8 characters.");
    if (password !== confirmPassword) return setError("Passwords do not match.");

    const modelToSave = slugFromSelection(selection);

    setLoading(true);
    try {
      const result = await invoke<CreateResult>("create_wallet", {
        password,
        nodeName: nodeName.trim(),
        model: modelToSave,
        // llmUrl is no longer used (endpoints are hardcoded per provider).
        // We keep the parameter on the IPC call for one release so older
        // Tauri builds receive a defined value; the node-side `wallet-create`
        // command logs a deprecation WARN and ignores it.
        llmUrl: null,
        llmKey: needsCredentials ? llmKey.trim() : null,
      });

      if (!result.success || !result.wallet_address) {
        setError(result.error_message ?? "Wallet creation failed.");
        setLoading(false);
        return;
      }

      onCreated(password, result.wallet_address);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setLoading(false);
    }
  };

  const providerSelectValue = selection.kind === "ollama" ? "ollama" : selection.provider;
  const onProviderChange = (next: string) => {
    if (next === "ollama") {
      setSelection({ kind: "ollama", modelId: OLLAMA_DEFAULT_MODELS[0].modelId });
      return;
    }
    const entry = CLOUD_PROVIDERS_UI.find(p => p.id === next);
    if (!entry) return;
    setSelection({ kind: "cloud", provider: entry.id, tier: "mid" });
  };

  return (
    <div className="flex flex-col items-center justify-center min-h-screen text-slate-100 py-10">
      <div className="w-full max-w-2xl p-8 space-y-6">
        <div className="text-center space-y-2">
          <div className="w-20 h-20 mx-auto flex items-center justify-center">
            <img
              src="/synapseia-logo.png"
              alt="Synapseia"
              className="w-full h-full object-contain drop-shadow-[0_0_40px_rgba(0,212,255,0.25)]"
            />
          </div>
          <h1 className="text-3xl font-bold text-slate-100 tracking-tight">Create Your Node</h1>
          <p className="text-slate-400 text-sm max-w-md mx-auto">
            First-time setup. Pick a password now — it encrypts your wallet locally and cannot be recovered.
          </p>
        </div>

        <Card padding="md" className="space-y-4">
          <Input
            label="Node Name"
            value={nodeName}
            onChange={(e) => setNodeName(e.target.value)}
            placeholder="My Synapseia Node"
            disabled={loading}
            hint="Visible to other peers in the network"
          />

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <Input
              label="Wallet Password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="min 8 characters"
              disabled={loading}
            />
            <Input
              label="Confirm Password"
              type="password"
              value={confirmPassword}
              onChange={(e) => setConfirmPassword(e.target.value)}
              placeholder="repeat password"
              disabled={loading}
              onKeyDown={(e) => e.key === "Enter" && !loading && handleCreate()}
            />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div className="space-y-1.5">
              <label className="block text-xs uppercase tracking-wide text-slate-400">Provider</label>
              <select
                value={providerSelectValue}
                onChange={(e) => onProviderChange(e.target.value)}
                disabled={loading}
                className="w-full px-4 py-2.5 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06] rounded-lg text-slate-100 focus:outline-none focus:border-[var(--accent-cyan)]/60 focus:ring-2 focus:ring-[var(--accent-cyan)]/20 disabled:opacity-60 transition-all"
              >
                <option value="ollama">Ollama (local)</option>
                {CLOUD_PROVIDERS_UI.map((p) => (
                  <option key={p.id} value={p.id}>{p.label}</option>
                ))}
              </select>
            </div>
            <div className="space-y-1.5">
              <label className="block text-xs uppercase tracking-wide text-slate-400">
                {selection.kind === "ollama" ? "Local model" : "Tier"}
              </label>
              {selection.kind === "ollama" ? (
                <select
                  value={selection.modelId}
                  onChange={(e) => setSelection({ kind: "ollama", modelId: e.target.value })}
                  disabled={loading}
                  className="w-full px-4 py-2.5 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06] rounded-lg text-slate-100 focus:outline-none focus:border-[var(--accent-cyan)]/60 focus:ring-2 focus:ring-[var(--accent-cyan)]/20 disabled:opacity-60 transition-all"
                >
                  {OLLAMA_DEFAULT_MODELS.map((m) => (
                    <option key={m.modelId} value={m.modelId}>
                      {m.modelId}{m.hint ? ` — ${m.hint}` : ""}
                    </option>
                  ))}
                </select>
              ) : (
                <select
                  value={selection.tier}
                  onChange={(e) =>
                    setSelection({ kind: "cloud", provider: selection.provider, tier: e.target.value as ModelTier })
                  }
                  disabled={loading}
                  className="w-full px-4 py-2.5 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06] rounded-lg text-slate-100 focus:outline-none focus:border-[var(--accent-cyan)]/60 focus:ring-2 focus:ring-[var(--accent-cyan)]/20 disabled:opacity-60 transition-all"
                >
                  {(["top", "mid", "budget"] as const).map((tier) => {
                    const entry = CLOUD_PROVIDERS_UI.find(p => p.id === selection.provider)!;
                    const desc = entry.models[tier];
                    const tierLabel = tier === "top" ? "Top" : tier === "mid" ? "Mid" : "Budget";
                    return (
                      <option key={tier} value={tier}>
                        {tierLabel} — {desc.modelId}{desc.hint ? ` (${desc.hint})` : ""}
                      </option>
                    );
                  })}
                </select>
              )}
            </div>
          </div>

          {needsCredentials && (
            <Input
              label="LLM API Key"
              type="password"
              value={llmKey}
              onChange={(e) => setLlmKey(e.target.value)}
              placeholder="sk-..."
              disabled={loading}
              hint={`Or set env var ${CLOUD_PROVIDERS_UI.find(p => selection.kind === "cloud" && p.id === selection.provider)?.apiKeyEnvVar ?? ""}`}
            />
          )}

          {error && (
            <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-3">
              <p className="text-sm text-red-300 break-words">{error}</p>
            </div>
          )}

          <Button variant="primary" size="lg" onClick={handleCreate} disabled={loading} className="w-full">
            {loading ? "Creating wallet…" : "Create Node"}
          </Button>

          <p className="text-xs text-slate-500 text-center">
            Your wallet is encrypted with AES-256-GCM and stored at ~/.synapseia. No password ever leaves your machine.
          </p>
        </Card>
      </div>
    </div>
  );
}
