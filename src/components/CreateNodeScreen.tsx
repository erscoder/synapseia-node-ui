import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input } from "./ui";

interface Props {
  onCreated: (password: string, walletAddress: string) => void;
}

interface CreateResult {
  success: boolean;
  wallet_address: string | null;
  error_code: string | null;
  error_message: string | null;
}

const POPULAR_MODELS = [
  { id: "openai/gpt-4o", name: "OpenAI GPT-4o" },
  { id: "anthropic/claude-3-5-sonnet", name: "Anthropic Claude-3.5 Sonnet" },
  { id: "google/gemini-1.5-pro", name: "Google Gemini-1.5 Pro" },
  { id: "minimax/abab6-chat", name: "Minimax" },
  { id: "ollama/llama3", name: "Ollama Llama3 (local)" },
  { id: "ollama/qwen2.5", name: "Ollama Qwen2.5 (local)" },
  { id: "custom", name: "Custom…" },
];

export function CreateNodeScreen({ onCreated }: Props) {
  const [nodeName, setNodeName] = useState("");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [coordinatorUrl, setCoordinatorUrl] = useState("http://localhost:3701");
  const [selectedModel, setSelectedModel] = useState("openai/gpt-4o");
  const [customModel, setCustomModel] = useState("");
  const [llmUrl, setLlmUrl] = useState("");
  const [llmKey, setLlmKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const isLocalModel = selectedModel.startsWith("ollama/");
  const needsCredentials = !isLocalModel && selectedModel !== "custom";

  const handleCreate = async () => {
    setError(null);

    if (!nodeName.trim()) return setError("Node name is required.");
    if (password.length < 8) return setError("Password must be at least 8 characters.");
    if (password !== confirmPassword) return setError("Passwords do not match.");
    if (!coordinatorUrl.startsWith("http://") && !coordinatorUrl.startsWith("https://")) {
      return setError("Coordinator URL must start with http:// or https://.");
    }

    const modelToSave = selectedModel === "custom" ? customModel.trim() : selectedModel;
    if (!modelToSave) return setError("Please select or enter a model.");

    setLoading(true);
    try {
      const result = await invoke<CreateResult>("create_wallet", {
        password,
        nodeName: nodeName.trim(),
        coordinatorUrl: coordinatorUrl.trim(),
        model: modelToSave,
        llmUrl: needsCredentials ? llmUrl.trim() : null,
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

          <Input
            label="Coordinator URL"
            value={coordinatorUrl}
            onChange={(e) => setCoordinatorUrl(e.target.value)}
            placeholder="http://localhost:3701"
            disabled={loading}
          />

          <div className="space-y-1.5">
            <label className="block text-xs uppercase tracking-wide text-slate-400">Default Model</label>
            <select
              value={selectedModel}
              onChange={(e) => setSelectedModel(e.target.value)}
              disabled={loading}
              className="w-full px-4 py-2.5 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06] rounded-lg text-slate-100 focus:outline-none focus:border-[var(--accent-cyan)]/60 focus:ring-2 focus:ring-[var(--accent-cyan)]/20 disabled:opacity-60 transition-all"
            >
              {POPULAR_MODELS.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
            {selectedModel === "custom" && (
              <Input
                value={customModel}
                onChange={(e) => setCustomModel(e.target.value)}
                placeholder="e.g. openai/gpt-4o"
                disabled={loading}
                className="mt-2"
              />
            )}
          </div>

          {needsCredentials && (
            <>
              <Input
                label="LLM API URL"
                value={llmUrl}
                onChange={(e) => setLlmUrl(e.target.value)}
                placeholder="https://api.openai.com/v1"
                disabled={loading}
              />
              <Input
                label="LLM API Key"
                type="password"
                value={llmKey}
                onChange={(e) => setLlmKey(e.target.value)}
                placeholder="sk-..."
                disabled={loading}
              />
            </>
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
