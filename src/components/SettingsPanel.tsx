import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CommandResult } from "../App";
import { Save, RotateCcw, Globe, Cpu, Key, Eye, EyeOff } from "lucide-react";
import { Card, Button, PageHeader, Input } from "./ui";

interface Props {
  password: string | null;
}

interface ConfigState {
  name: string;
  coordinatorUrl: string;
  defaultModel: string;
  llmUrl: string;
  llmKey: string;
  inferenceEnabled: boolean;
  inferenceModels: string;
}

const POPULAR_MODELS = [
  { id: "openai/gpt-4o", name: "OpenAI GPT-4o" },
  { id: "anthropic/claude-3-5-sonnet", name: "Anthropic Claude-3.5 Sonnet" },
  { id: "google/gemini-1.5-pro", name: "Google Gemini-1.5 Pro" },
  { id: "minimax/abab6-chat", name: "Minimax" },
  { id: "ollama/llama3", name: "Ollama Llama3" },
  { id: "ollama/qwen2.5", name: "Ollama Qwen2.5" },
  { id: "custom", name: "Custom…" },
];

const DEFAULT_CONFIG: ConfigState = {
  name: "",
  coordinatorUrl: "http://localhost:3701",
  defaultModel: "ollama/qwen2.5:0.5b",
  llmUrl: "",
  llmKey: "",
  inferenceEnabled: false,
  inferenceModels: "",
};

// The CLI prefixes every logger.log line with an ANSI timestamp + level
// (e.g. `\x1b[90m02:10:58.241\x1b[0m  \x1b[32mINFO\x1b[0m  <msg>`). The
// actual JSON payload lives inside multiple such lines. Extract the
// substring from the first `{` to the last `}` and strip ANSI on that
// window, then parse. Anything else is noise.
function extractConfigJson(raw: string): Record<string, unknown> | null {
  if (!raw) return null;
  const firstBrace = raw.indexOf("{");
  const lastBrace = raw.lastIndexOf("}");
  if (firstBrace < 0 || lastBrace <= firstBrace) return null;
  const slice = raw.slice(firstBrace, lastBrace + 1);
  // Drop ANSI escape sequences and any log-prefix tails that snuck into the
  // brace span (e.g. `... 02:10:58.241  INFO  "name"`).
  const cleaned = slice
    // eslint-disable-next-line no-control-regex
    .replace(/\x1b\[[\d;]*m/g, "")
    .replace(/^\s*\d{1,2}:\d{1,2}:\d{1,2}(?:\.\d{1,3})?\s+(?:INFO|WARN|ERROR|DEBUG|TRACE|FATAL)\s+/gm, "");
  try {
    return JSON.parse(cleaned) as Record<string, unknown>;
  } catch {
    return null;
  }
}

function pickModelId(defaultModel: string | undefined): {
  selectedId: string;
  custom: string;
} {
  if (!defaultModel) return { selectedId: "ollama/qwen2.5", custom: "" };
  const match = POPULAR_MODELS.find((m) => m.id === defaultModel);
  if (match) return { selectedId: match.id, custom: "" };
  return { selectedId: "custom", custom: defaultModel };
}

export function SettingsPanel({ password }: Props) {
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [config, setConfig] = useState<ConfigState>(DEFAULT_CONFIG);
  const [selectedModel, setSelectedModel] = useState("ollama/qwen2.5");
  const [customModel, setCustomModel] = useState("");
  // Show API key as dots until the user opts in, but track whether one is
  // configured so the UI can signal "set" without revealing the value.
  const [showKey, setShowKey] = useState(false);
  const [hasStoredKey, setHasStoredKey] = useState(false);

  const loadConfig = async () => {
    if (!password) return;
    setLoading(true);
    setError(null);
    try {
      const res = await invoke<CommandResult>("run_command", {
        command: "config",
        args: ["--show"],
        password,
      });
      if (!res.success || !res.output) {
        setError(res.error || "Failed to load config");
        return;
      }
      const parsed = extractConfigJson(res.output);
      if (!parsed) {
        setError(`Could not parse config output.\n\n${res.output.slice(0, 400)}`);
        return;
      }
      const { selectedId, custom } = pickModelId(parsed.defaultModel as string | undefined);
      setSelectedModel(selectedId);
      setCustomModel(custom);
      const storedKey = typeof parsed.llmKey === "string" ? parsed.llmKey : "";
      setHasStoredKey(storedKey.length > 0);
      setConfig({
        name: (parsed.name as string) ?? "",
        coordinatorUrl: (parsed.coordinatorUrl as string) ?? DEFAULT_CONFIG.coordinatorUrl,
        defaultModel: (parsed.defaultModel as string) ?? DEFAULT_CONFIG.defaultModel,
        llmUrl: (parsed.llmUrl as string) ?? "",
        // Don't surface the raw key — keep it in memory only when the user
        // clicks "Show" or actively types a new one.
        llmKey: storedKey,
        inferenceEnabled: (parsed.inferenceEnabled as boolean) ?? false,
        inferenceModels: Array.isArray(parsed.inferenceModels)
          ? (parsed.inferenceModels as string[]).join(", ")
          : "",
      });
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void loadConfig();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [password]);

  const saveConfig = async () => {
    if (!password) return;
    setSaving(true);
    setError(null);
    setSuccess(null);
    try {
      const modelToSave = selectedModel === "custom" ? customModel : selectedModel;
      const updates: { flag: string; value: string }[] = [];
      if (config.name) updates.push({ flag: "--set-name", value: config.name });
      if (config.coordinatorUrl) updates.push({ flag: "--set-coordinator-url", value: config.coordinatorUrl });
      if (modelToSave) updates.push({ flag: "--set-model", value: modelToSave });
      if (config.llmUrl) updates.push({ flag: "--set-llm-url", value: config.llmUrl });
      // Only push the key if the user actually typed something (not the
      // placeholder dots or the value we loaded back from disk).
      if (config.llmKey && config.llmKey !== "__STORED__") {
        updates.push({ flag: "--set-llm-key", value: config.llmKey });
      }

      for (const u of updates) {
        await invoke<CommandResult>("run_command", {
          command: "config",
          args: [u.flag, u.value],
          password,
        });
      }

      setSuccess("Config saved successfully");
      // Reload so the UI reflects what's actually on disk now.
      await loadConfig();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const resetConfig = () => {
    setConfig(DEFAULT_CONFIG);
    setSelectedModel("ollama/qwen2.5");
    setCustomModel("");
    setHasStoredKey(false);
  };

  const updateField = (field: keyof ConfigState, value: string | boolean) => {
    setConfig((prev) => ({ ...prev, [field]: value }));
  };

  const keyDisplayValue = showKey
    ? config.llmKey
    : config.llmKey
      ? "•".repeat(Math.min(config.llmKey.length, 32))
      : "";

  return (
    <div className="space-y-6">
      <PageHeader
        title="Settings"
        subtitle="Node identity, LLM credentials, and inference options"
        action={
          <>
            <Button variant="secondary" onClick={loadConfig} disabled={loading || !password}>
              <RotateCcw className={`w-4 h-4 ${loading ? "animate-spin" : ""}`} />
              {loading ? "Loading…" : "Reload"}
            </Button>
            <Button variant="primary" onClick={saveConfig} disabled={saving || !password}>
              <Save className="w-4 h-4" />
              {saving ? "Saving…" : "Save"}
            </Button>
          </>
        }
      />

      <Card padding="md">
        <div className="flex items-center gap-2 mb-4">
          <Cpu className="w-5 h-5 text-[var(--accent-cyan)]" />
          <h2 className="text-lg font-semibold text-slate-100">Node Identity</h2>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <Input
            label="Node Name"
            value={config.name}
            onChange={(e) => updateField("name", e.target.value)}
            placeholder="My Synapseia Node"
            hint="Visible to other peers in the network"
          />
          <Input
            label="Coordinator URL"
            value={config.coordinatorUrl}
            onChange={(e) => updateField("coordinatorUrl", e.target.value)}
            placeholder="http://localhost:3701"
            hint="URL of the coordinator server"
          />
        </div>
      </Card>

      <Card padding="md">
        <div className="flex items-center gap-2 mb-4">
          <Key className="w-5 h-5 text-[var(--accent-purple)]" />
          <h2 className="text-lg font-semibold text-slate-100">LLM (Cloud) Settings</h2>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div className="space-y-1.5">
            <label className="block text-xs uppercase tracking-wide text-slate-400">Default Model</label>
            {/*
              appearance-none + custom chevron keeps the select flush with
              the sibling Input widths; the native macOS chevron arrow would
              otherwise force the control narrower than its grid column.
            */}
            <select
              value={selectedModel}
              onChange={(e) => setSelectedModel(e.target.value)}
              className="w-full px-4 py-2.5 pr-10 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06] rounded-lg text-slate-100 focus:outline-none focus:border-[var(--accent-purple)]/60 focus:ring-2 focus:ring-[var(--accent-purple)]/20 transition-all appearance-none bg-no-repeat bg-[length:1.25rem] bg-[position:right_0.75rem_center] cursor-pointer"
              style={{
                backgroundImage:
                  "url(\"data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20' viewBox='0 0 24 24' fill='none' stroke='%2394a3b8' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E\")",
              }}
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
                className="mt-2"
              />
            )}
            <p className="text-xs text-slate-500">Format: provider/modelname</p>
          </div>
          <Input
            label="LLM API URL"
            value={config.llmUrl}
            onChange={(e) => updateField("llmUrl", e.target.value)}
            placeholder="https://api.openai.com/v1"
            hint="Base URL for cloud LLM API (optional)"
          />
          <div className="md:col-span-2 space-y-1.5">
            <label className="block text-xs uppercase tracking-wide text-slate-400">LLM API Key</label>
            <div className="flex gap-2">
              <input
                type={showKey ? "text" : "password"}
                value={keyDisplayValue}
                onChange={(e) => updateField("llmKey", e.target.value)}
                placeholder={hasStoredKey ? "••••••••••••  (configured)" : "sk-..."}
                className="flex-1 px-4 py-2.5 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06] rounded-lg text-slate-100 placeholder-slate-500 focus:outline-none focus:border-[var(--accent-purple)]/60 focus:ring-2 focus:ring-[var(--accent-purple)]/20 transition-all font-mono"
              />
              <Button variant="ghost" onClick={() => setShowKey((v) => !v)} disabled={!config.llmKey}>
                {showKey ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
              </Button>
            </div>
            <p className="text-xs text-slate-500">
              {hasStoredKey
                ? "A key is configured. Click the eye to reveal, or type a new one to replace it."
                : "API key for cloud LLM provider (optional)"}
            </p>
          </div>
        </div>
      </Card>

      <Card padding="md">
        <div className="flex items-center gap-2 mb-4">
          <Globe className="w-5 h-5 text-emerald-400" />
          <h2 className="text-lg font-semibold text-slate-100">Inference Server</h2>
        </div>
        <label className="flex items-center gap-3 cursor-pointer">
          <input
            type="checkbox"
            checked={config.inferenceEnabled}
            onChange={(e) => updateField("inferenceEnabled", e.target.checked)}
            className="rounded border-white/[0.06] bg-[var(--bg-elevated)] text-emerald-400 focus:ring-emerald-400/30"
          />
          <span className="text-sm text-slate-300">
            Enable GPU inference server (expose your GPU as an AI inference provider)
          </span>
        </label>
        {config.inferenceEnabled && (
          <div className="mt-4">
            <Input
              label="Inference Models"
              value={config.inferenceModels}
              onChange={(e) => updateField("inferenceModels", e.target.value)}
              placeholder="ollama/qwen2.5:7b, ollama/llama3:8b"
              hint="Comma-separated list of models to serve"
            />
          </div>
        )}
      </Card>

      <div className="flex items-center justify-between">
        <Button variant="ghost" onClick={resetConfig}>
          <RotateCcw className="w-4 h-4" />
          Reset fields
        </Button>
      </div>

      {error && (
        <Card padding="sm" className="border-red-500/30 bg-red-500/10">
          <p className="text-sm text-red-300 whitespace-pre-wrap break-words font-mono">{error}</p>
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
