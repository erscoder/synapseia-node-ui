import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CommandResult } from "../App";
import { Save, RotateCcw, Globe, Cpu, Key, Eye, EyeOff } from "lucide-react";
// `Globe` is still used by the Inference Server card; the Coordinator URL
// input that previously needed an icon is gone.
import { Card, Button, PageHeader, Input } from "./ui";
import {
  CLOUD_PROVIDERS_UI,
  OLLAMA_DEFAULT_MODELS,
  buildSlug,
  parseSlug,
  type CloudProviderId,
  type ModelTier,
} from "../lib/providers";

interface Props {
  password: string | null;
}

interface ConfigState {
  name: string;
  defaultModel: string;
  llmKey: string;
  inferenceEnabled: boolean;
  inferenceModels: string;
  /**
   * Optional override for the local Ollama endpoint. Empty string ("") means
   * "use the CLI's hardcoded default (http://localhost:11434)" — it is NEVER
   * persisted as a literal fallback. Stored separately from the node CLI's
   * `config.json` (Tauri-side `~/.synapseia/ui-settings.json`); injected into
   * spawned CLI invocations as the `OLLAMA_URL` env var.
   */
  ollamaUrl: string;
}

const DEFAULT_CONFIG: ConfigState = {
  name: "",
  defaultModel: "ollama/qwen2.5:0.5b",
  llmKey: "",
  inferenceEnabled: false,
  inferenceModels: "",
  ollamaUrl: "",
};

/**
 * Soft URL shape validation. Empty string is "use default" (valid).
 * Must start with http(s):// and must NOT end with a trailing slash.
 */
function isValidOllamaUrl(value: string): boolean {
  const trimmed = value.trim();
  if (trimmed === "") return true;
  if (!/^https?:\/\//i.test(trimmed)) return false;
  if (trimmed.endsWith("/")) return false;
  return true;
}

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

type ProviderSelection =
  | { kind: "cloud"; provider: CloudProviderId; tier: ModelTier }
  | { kind: "ollama"; modelId: string };

const DEFAULT_SELECTION: ProviderSelection = { kind: "ollama", modelId: "qwen2.5:0.5b" };

/**
 * Map a raw slug from disk to the (provider, tier) pair that drives the
 * two dropdowns. Slugs that no longer match the whitelist (post-migration
 * or user typos) fall back to the default Ollama tier — the node CLI's
 * config-load migration writes a corrected slug back to disk on the next
 * boot, so this UI fallback is just a one-render guard.
 */
function selectionFromSlug(slug: string | undefined): ProviderSelection {
  if (!slug) return DEFAULT_SELECTION;
  const parsed = parseSlug(slug);
  if (!parsed) return DEFAULT_SELECTION;
  if (parsed.provider === "ollama") {
    return { kind: "ollama", modelId: parsed.modelId };
  }
  // For cloud providers the tier may be missing if the operator picked a
  // model the UI doesn't offer. Snap to "top" so the dropdown shows
  // something coherent and the next save persists a known slug.
  return { kind: "cloud", provider: parsed.provider, tier: parsed.tier ?? "top" };
}

function slugFromSelection(sel: ProviderSelection): string {
  if (sel.kind === "ollama") return buildSlug("ollama", sel.modelId);
  const entry = CLOUD_PROVIDERS_UI.find(p => p.id === sel.provider);
  if (!entry) return DEFAULT_CONFIG.defaultModel;
  return buildSlug(entry.id, entry.models[sel.tier].modelId);
}

export function SettingsPanel({ password }: Props) {
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [config, setConfig] = useState<ConfigState>(DEFAULT_CONFIG);
  const [selection, setSelection] = useState<ProviderSelection>(DEFAULT_SELECTION);
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
      const slug = (parsed.defaultModel as string) ?? DEFAULT_CONFIG.defaultModel;
      setSelection(selectionFromSlug(slug));
      const storedKey = typeof parsed.llmKey === "string" ? parsed.llmKey : "";
      setHasStoredKey(storedKey.length > 0);
      // Fetch the Tauri-side UI settings (Ollama endpoint override) in
      // parallel. A failure here is non-fatal: we just fall back to the
      // empty string ("use default") and let the operator retype it.
      let ollamaUrl = "";
      try {
        const ui = await invoke<{ ollamaUrl?: string }>("get_ui_settings");
        if (ui && typeof ui.ollamaUrl === "string") ollamaUrl = ui.ollamaUrl;
      } catch {
        // Ignore — older Tauri builds without the command should not block
        // the rest of the config from loading.
      }
      setConfig({
        name: (parsed.name as string) ?? "",
        defaultModel: slug,
        llmKey: storedKey,
        inferenceEnabled: (parsed.inferenceEnabled as boolean) ?? false,
        inferenceModels: Array.isArray(parsed.inferenceModels)
          ? (parsed.inferenceModels as string[]).join(", ")
          : "",
        ollamaUrl,
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
    // Block save when the operator-typed Ollama URL is malformed; the same
    // shape check runs Rust-side as a defense in depth, but failing fast
    // here gives a clearer in-UI error.
    if (!isValidOllamaUrl(config.ollamaUrl)) {
      setError(
        "Invalid Ollama endpoint. Must start with http:// or https:// and have no trailing slash.",
      );
      return;
    }
    setSaving(true);
    setError(null);
    setSuccess(null);
    try {
      const modelToSave = slugFromSelection(selection);
      const updates: { flag: string; value: string }[] = [];
      if (config.name) updates.push({ flag: "--set-name", value: config.name });
      if (modelToSave) updates.push({ flag: "--set-model", value: modelToSave });
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

      // Persist the Ollama endpoint override via the Tauri-side store. Empty
      // string is intentionally preserved as "use default" — Rust trims and
      // skips the env var when blank. We always send the call so that
      // clearing the field overwrites a previously saved value.
      await invoke("set_ui_settings", { ollamaUrl: config.ollamaUrl.trim() });

      setSuccess("Config saved successfully");
      await loadConfig();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const resetConfig = () => {
    setConfig(DEFAULT_CONFIG);
    setSelection(DEFAULT_SELECTION);
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

  // Provider dropdown values: each cloud provider id, plus "ollama".
  // We use a sentinel string instead of an enum so the change handler
  // can branch by string equality without re-deriving union types.
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

  // Tier dropdown only appears for cloud providers; for ollama we show
  // the curated local-model list instead. The model id (vendor name) is
  // shown in parentheses so an operator can confirm which weight family
  // they're enabling without leaving the screen.
  const renderModelControl = () => {
    if (selection.kind === "ollama") {
      return (
        <select
          value={selection.modelId}
          onChange={(e) => setSelection({ kind: "ollama", modelId: e.target.value })}
          className="w-full px-4 py-2.5 pr-10 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06] rounded-lg text-slate-100 focus:outline-none focus:border-[var(--accent-purple)]/60 focus:ring-2 focus:ring-[var(--accent-purple)]/20 transition-all appearance-none bg-no-repeat bg-[length:1.25rem] bg-[position:right_0.75rem_center] cursor-pointer"
          style={{
            backgroundImage:
              "url(\"data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20' viewBox='0 0 24 24' fill='none' stroke='%2394a3b8' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E\")",
          }}
        >
          {OLLAMA_DEFAULT_MODELS.map((m) => (
            <option key={m.modelId} value={m.modelId}>
              {m.modelId}{m.hint ? ` — ${m.hint}` : ""}
            </option>
          ))}
        </select>
      );
    }
    const entry = CLOUD_PROVIDERS_UI.find(p => p.id === selection.provider);
    if (!entry) return null;
    return (
      <select
        value={selection.tier}
        onChange={(e) => setSelection({ kind: "cloud", provider: selection.provider, tier: e.target.value as ModelTier })}
        className="w-full px-4 py-2.5 pr-10 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06] rounded-lg text-slate-100 focus:outline-none focus:border-[var(--accent-purple)]/60 focus:ring-2 focus:ring-[var(--accent-purple)]/20 transition-all appearance-none bg-no-repeat bg-[length:1.25rem] bg-[position:right_0.75rem_center] cursor-pointer"
        style={{
          backgroundImage:
            "url(\"data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20' viewBox='0 0 24 24' fill='none' stroke='%2394a3b8' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E\")",
        }}
      >
        {(["top", "mid", "budget"] as const).map((tier) => {
          const desc = entry.models[tier];
          const tierLabel = tier === "top" ? "Top" : tier === "mid" ? "Mid" : "Budget";
          return (
            <option key={tier} value={tier}>
              {tierLabel} — {desc.modelId}{desc.hint ? ` (${desc.hint})` : ""}
            </option>
          );
        })}
      </select>
    );
  };

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
        <Input
          label="Node Name"
          value={config.name}
          onChange={(e) => updateField("name", e.target.value)}
          placeholder="My Synapseia Node"
          hint="Visible to other peers in the network"
        />
      </Card>

      <Card padding="md">
        <div className="flex items-center gap-2 mb-4">
          <Key className="w-5 h-5 text-[var(--accent-purple)]" />
          <h2 className="text-lg font-semibold text-slate-100">LLM Provider</h2>
        </div>
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div className="space-y-1.5">
            <label className="block text-xs uppercase tracking-wide text-slate-400">Provider</label>
            <select
              value={providerSelectValue}
              onChange={(e) => onProviderChange(e.target.value)}
              className="w-full px-4 py-2.5 pr-10 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border border-white/[0.06] rounded-lg text-slate-100 focus:outline-none focus:border-[var(--accent-purple)]/60 focus:ring-2 focus:ring-[var(--accent-purple)]/20 transition-all appearance-none bg-no-repeat bg-[length:1.25rem] bg-[position:right_0.75rem_center] cursor-pointer"
              style={{
                backgroundImage:
                  "url(\"data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20' viewBox='0 0 24 24' fill='none' stroke='%2394a3b8' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E\")",
              }}
            >
              <option value="ollama">Ollama (local)</option>
              {CLOUD_PROVIDERS_UI.map((p) => (
                <option key={p.id} value={p.id}>{p.label}</option>
              ))}
            </select>
            <p className="text-xs text-slate-500">
              {selection.kind === "ollama"
                ? "Endpoint is editable below."
                : "Cloud endpoints are hardcoded per provider."}
            </p>
          </div>
          <div className="space-y-1.5">
            <label className="block text-xs uppercase tracking-wide text-slate-400">
              {selection.kind === "ollama" ? "Local model" : "Tier"}
            </label>
            {renderModelControl()}
            <p className="text-xs text-slate-500">
              {selection.kind === "ollama"
                ? "Pulled via 'ollama pull <model>'."
                : "Top = strongest, Mid = balanced, Budget = fastest/cheapest."}
            </p>
          </div>
          {selection.kind === "ollama" && (
            <div className="md:col-span-2 space-y-1.5">
              <label className="block text-xs uppercase tracking-wide text-slate-400">Endpoint</label>
              <input
                type="text"
                value={config.ollamaUrl}
                onChange={(e) => updateField("ollamaUrl", e.target.value)}
                placeholder="http://localhost:11434"
                className={`w-full px-4 py-2.5 bg-[var(--bg-elevated)]/80 backdrop-blur-sm border rounded-lg text-slate-100 placeholder-slate-500 focus:outline-none focus:ring-2 transition-all font-mono ${
                  isValidOllamaUrl(config.ollamaUrl)
                    ? "border-white/[0.06] focus:border-[var(--accent-purple)]/60 focus:ring-[var(--accent-purple)]/20"
                    : "border-red-500/60 focus:border-red-500/80 focus:ring-red-500/30"
                }`}
              />
              <p className={`text-xs ${isValidOllamaUrl(config.ollamaUrl) ? "text-slate-500" : "text-red-300"}`}>
                {isValidOllamaUrl(config.ollamaUrl)
                  ? "Leave blank for the default. Use this to point at a Docker container or remote host (e.g. http://localhost:11435)."
                  : "Must start with http:// or https:// and must not end with a trailing slash."}
              </p>
            </div>
          )}
          {selection.kind === "cloud" && (
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
                  : `API key for ${CLOUD_PROVIDERS_UI.find(p => p.id === selection.provider)?.label ?? "selected provider"}. ` +
                    `Env var: ${CLOUD_PROVIDERS_UI.find(p => p.id === selection.provider)?.apiKeyEnvVar ?? ""}.`}
              </p>
            </div>
          )}
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
