import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import { SettingsPanel } from "./SettingsPanel";

// Hoisted mock state so the @tauri-apps/api/core mock can read it.
const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

interface CliConfig {
  name: string;
  defaultModel: string;
  llmKey: string;
  inferenceEnabled: boolean;
  inferenceModels: string[];
}

function configShowOutput(cfg: CliConfig): string {
  return JSON.stringify(cfg);
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe("SettingsPanel saveConfig", () => {
  it("forwards the full 4-arg shape to set_ui_settings when saving", async () => {
    const initial: CliConfig = {
      name: "test-node",
      defaultModel: "nvidia/meta/llama-3.3-70b-instruct",
      llmKey: "__STORED__",
      inferenceEnabled: false,
      inferenceModels: [],
    };
    let setUiSettingsArgs: Record<string, unknown> | null = null;
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "run_command") {
        // config --show on initial load, then any config --set-* call on save.
        const subArgs = (args?.args as string[]) ?? [];
        if (subArgs.includes("--show")) {
          return Promise.resolve({
            success: true,
            output: configShowOutput(initial),
            error: null,
          });
        }
        return Promise.resolve({ success: true, output: "ok", error: null });
      }
      if (cmd === "get_ui_settings") {
        return Promise.resolve({ ollamaUrl: "" });
      }
      if (cmd === "set_ui_settings") {
        setUiSettingsArgs = args ?? {};
        return Promise.resolve({});
      }
      return Promise.resolve(null);
    });

    render(<SettingsPanel password="hunter2" />);

    // Wait for the initial config load to finish.
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "run_command",
        expect.objectContaining({ command: "config" }),
      );
    });

    // Trigger save by clicking the Save button.
    const saveBtn = await screen.findByRole("button", { name: /save/i });
    await act(async () => {
      saveBtn.click();
    });

    await waitFor(() => {
      expect(setUiSettingsArgs).not.toBeNull();
    });

    // The set_ui_settings call MUST carry all four fields so the Rust
    // side can persist the cloud provider selection. This is the core
    // assertion that protects against the original "ollama_url only"
    // regression that caused the UI to revert to Ollama on Windows.
    expect(setUiSettingsArgs).toMatchObject({
      ollamaUrl: expect.any(String),
      llmProvider: expect.any(String),
      llmModelSlug: expect.any(String),
    });
    // llmApiKey is allowed to be null when the operator has not retyped
    // a new key (the previously stored value is preserved Rust-side).
    expect(setUiSettingsArgs).toHaveProperty("llmApiKey");
  });

  it("surfaces a CLI run_command failure and does NOT call loadConfig afterwards", async () => {
    const initial: CliConfig = {
      name: "",
      defaultModel: "nvidia/meta/llama-3.3-70b-instruct",
      llmKey: "",
      inferenceEnabled: false,
      inferenceModels: [],
    };
    let configShowCalls = 0;
    let setUiSettingsCalled = false;
    invokeMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "run_command") {
        const subArgs = (args?.args as string[]) ?? [];
        if (subArgs.includes("--show")) {
          configShowCalls += 1;
          return Promise.resolve({
            success: true,
            output: configShowOutput(initial),
            error: null,
          });
        }
        // Simulate the Windows failure: --set-model returns success=false
        // with no exception. Previously the save path ignored this and
        // re-loaded the CLI config, making the UI appear to revert.
        return Promise.resolve({
          success: false,
          output: "",
          error: "Windows wallet password capture timed out",
        });
      }
      if (cmd === "get_ui_settings") {
        return Promise.resolve({ ollamaUrl: "" });
      }
      if (cmd === "set_ui_settings") {
        setUiSettingsCalled = true;
        return Promise.resolve({});
      }
      return Promise.resolve(null);
    });

    render(<SettingsPanel password="hunter2" />);

    await waitFor(() => {
      expect(configShowCalls).toBe(1);
    });

    const saveBtn = await screen.findByRole("button", { name: /save/i });
    await act(async () => {
      saveBtn.click();
    });

    // The error message must surface to the operator. The component
    // renders the error inside a Card with class text-red-300, so just
    // check that the failure detail string is on screen.
    await waitFor(() => {
      expect(
        screen.getByText(/Windows wallet password capture timed out/i),
      ).toBeInTheDocument();
    });

    // Crucially: the CLI config must NOT be re-read after the failure.
    // If it were, the (still unchanged) CLI config would overwrite the
    // operator's typed selection and produce the silent revert bug.
    expect(configShowCalls).toBe(1);
    // And set_ui_settings must not run when an earlier CLI step failed.
    expect(setUiSettingsCalled).toBe(false);
  });
});
