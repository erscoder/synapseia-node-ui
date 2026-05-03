/**
 * Provider whitelist for the desktop UI.
 *
 * This mirrors `packages/node/src/modules/llm/providers.ts` (the
 * authoritative table) so the UI and the node agree on which slugs are
 * valid. The two files MUST stay in sync — when adding/removing a
 * provider here, also update the node table or migrations will rewrite
 * what the user just selected.
 */

export type CloudProviderId =
  | "openai"
  | "anthropic"
  | "google"
  | "moonshot"
  | "minimax"
  | "zhipu";

export type ModelTier = "top" | "mid" | "budget";

export interface UiModelDescriptor {
  modelId: string;
  /** Brief human description rendered next to the option (capacity, latency hint). */
  hint?: string;
}

export interface UiCloudProvider {
  id: CloudProviderId;
  label: string;
  /** Env var the operator can set instead of typing the key into the UI. */
  apiKeyEnvVar: string;
  models: Record<ModelTier, UiModelDescriptor>;
}

export const CLOUD_PROVIDERS_UI: readonly UiCloudProvider[] = [
  {
    id: "openai",
    label: "OpenAI",
    apiKeyEnvVar: "OPENAI_API_KEY",
    models: {
      top: { modelId: "gpt-5", hint: "flagship" },
      mid: { modelId: "gpt-4o", hint: "balanced" },
      budget: { modelId: "gpt-4o-mini", hint: "fast & cheap" },
    },
  },
  {
    id: "anthropic",
    label: "Anthropic",
    apiKeyEnvVar: "ANTHROPIC_API_KEY",
    models: {
      top: { modelId: "claude-opus-4-7", hint: "deepest reasoning" },
      mid: { modelId: "claude-sonnet-4-6", hint: "recommended default" },
      budget: { modelId: "claude-haiku-4-5", hint: "fast & cheap" },
    },
  },
  {
    id: "google",
    label: "Google (Gemini)",
    apiKeyEnvVar: "GEMINI_API_KEY",
    models: {
      top: { modelId: "gemini-2.5-pro", hint: "1M-token context" },
      mid: { modelId: "gemini-2.5-flash", hint: "balanced" },
      budget: { modelId: "gemini-2.5-flash-lite", hint: "fast & cheap" },
    },
  },
  {
    id: "moonshot",
    label: "Kimi (Moonshot)",
    apiKeyEnvVar: "MOONSHOT_API_KEY",
    models: {
      top: { modelId: "kimi-k2.6", hint: "long-form coding" },
      mid: { modelId: "kimi-k2-0711-preview", hint: "balanced" },
      budget: { modelId: "moonshot-v1-32k", hint: "32K window" },
    },
  },
  {
    id: "minimax",
    label: "MiniMax",
    apiKeyEnvVar: "MINIMAX_API_KEY",
    models: {
      top: { modelId: "MiniMax-M2.7", hint: "240K window" },
      mid: { modelId: "abab7-chat-preview", hint: "balanced" },
      budget: { modelId: "abab6.5s-chat", hint: "fast & cheap" },
    },
  },
  {
    id: "zhipu",
    label: "Zhipu (GLM)",
    apiKeyEnvVar: "ZHIPU_API_KEY",
    models: {
      top: { modelId: "glm-4.6", hint: "flagship" },
      mid: { modelId: "glm-4-plus", hint: "balanced" },
      budget: { modelId: "glm-4-flash", hint: "fast & cheap" },
    },
  },
];

export const OLLAMA_DEFAULT_MODELS: readonly UiModelDescriptor[] = [
  { modelId: "qwen2.5:0.5b", hint: "tiny — Tier 0" },
  { modelId: "qwen2.5:3b", hint: "small — Tier 1" },
  { modelId: "gemma3:4b", hint: "small — Tier 1+" },
  { modelId: "llama3.2:3b", hint: "balanced" },
];

export const FALLBACK_MODEL_SLUG = "anthropic/claude-sonnet-4-6";

/**
 * Parse a `provider/model` slug into the (provider, modelId) pair the UI
 * uses to drive the dropdowns. Returns null for `custom`, malformed
 * slugs, or providers no longer in the whitelist.
 */
export function parseSlug(
  slug: string,
): { provider: CloudProviderId | "ollama"; modelId: string; tier?: ModelTier } | null {
  if (!slug || !slug.includes("/")) return null;
  const slash = slug.indexOf("/");
  const provider = slug.slice(0, slash);
  const modelId = slug.slice(slash + 1);
  if (!modelId) return null;

  if (provider === "ollama") return { provider: "ollama", modelId };

  const entry = CLOUD_PROVIDERS_UI.find(p => p.id === provider);
  if (!entry) return null;

  let tier: ModelTier | undefined;
  for (const t of ["top", "mid", "budget"] as const) {
    if (entry.models[t].modelId === modelId) {
      tier = t;
      break;
    }
  }
  return { provider: entry.id, modelId, tier };
}

export function buildSlug(provider: CloudProviderId | "ollama", modelId: string): string {
  return `${provider}/${modelId}`;
}
