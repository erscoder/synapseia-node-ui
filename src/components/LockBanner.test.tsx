import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { LockBanner } from "./LockBanner";
import type { ExternalLockInfo } from "../hooks/useExternalLock";

function makeLock(overrides: Partial<ExternalLockInfo> = {}): ExternalLockInfo {
  return {
    pid: 16451,
    source: "cli",
    startedAt: "2026-05-17T10:00:00.000Z",
    ageSeconds: 720, // 12m
    isAlive: true,
    ...overrides,
  };
}

describe("<LockBanner />", () => {
  let onTakeOver: ReturnType<typeof vi.fn>;
  let onCleanStale: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    onTakeOver = vi.fn().mockResolvedValue(undefined);
    onCleanStale = vi.fn().mockResolvedValue(undefined);
  });

  it("renders CLI-source alive state with Take over action", () => {
    render(
      <LockBanner
        lock={makeLock({ source: "cli", isAlive: true })}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    expect(screen.getByText(/node already running from cli/i)).toBeInTheDocument();
    expect(screen.getByText(/pid 16451/i)).toBeInTheDocument();
    expect(screen.getByText(/12m/)).toBeInTheDocument();
    const btn = screen.getByRole("button", { name: /take over/i });
    expect(btn).toBeInTheDocument();
    expect(btn).not.toBeDisabled();
  });

  it("renders UI-source alive state with Stop other window action", () => {
    render(
      <LockBanner
        lock={makeLock({ source: "ui", isAlive: true, ageSeconds: 65 })}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    expect(screen.getByText(/node running in another window/i)).toBeInTheDocument();
    expect(screen.getByText(/1m/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /stop other window/i })).toBeInTheDocument();
  });

  it("renders dead (zombie) state with Clean lock action", () => {
    render(
      <LockBanner
        lock={makeLock({ isAlive: false })}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    expect(screen.getByText(/stale lock detected/i)).toBeInTheDocument();
    expect(
      screen.getByText(/previous node \(pid 16451\) crashed without cleanup/i),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /clean lock/i })).toBeInTheDocument();
  });

  it("has role=alert and aria-live=polite for accessibility", () => {
    render(
      <LockBanner
        lock={makeLock()}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    const alert = screen.getByRole("alert");
    expect(alert).toHaveAttribute("aria-live", "polite");
  });

  it("wraps the age in a <time> element with the ISO timestamp", () => {
    const startedAt = "2026-05-17T10:00:00.000Z";
    render(
      <LockBanner
        lock={makeLock({ startedAt })}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    // querySelector via container — testing-library doesn't expose time-element roles.
    const timeEl = document.querySelector("time");
    expect(timeEl).not.toBeNull();
    expect(timeEl?.getAttribute("dateTime")).toBe(startedAt);
  });

  it("calls onTakeOver when clicking Take over (alive cli)", async () => {
    render(
      <LockBanner
        lock={makeLock({ source: "cli", isAlive: true })}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    const btn = screen.getByRole("button", { name: /take over/i });
    await act(async () => {
      btn.click();
    });
    expect(onTakeOver).toHaveBeenCalledTimes(1);
    expect(onCleanStale).not.toHaveBeenCalled();
  });

  it("calls onTakeOver when clicking Stop other window (alive ui)", async () => {
    render(
      <LockBanner
        lock={makeLock({ source: "ui", isAlive: true })}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    const btn = screen.getByRole("button", { name: /stop other window/i });
    await act(async () => {
      btn.click();
    });
    expect(onTakeOver).toHaveBeenCalledTimes(1);
    expect(onCleanStale).not.toHaveBeenCalled();
  });

  it("calls onCleanStale when clicking Clean lock (dead)", async () => {
    render(
      <LockBanner
        lock={makeLock({ isAlive: false })}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    const btn = screen.getByRole("button", { name: /clean lock/i });
    await act(async () => {
      btn.click();
    });
    expect(onCleanStale).toHaveBeenCalledTimes(1);
    expect(onTakeOver).not.toHaveBeenCalled();
  });

  it("disables the action button while the handler is in-flight", async () => {
    let resolve: () => void = () => {};
    onTakeOver = vi.fn().mockImplementation(
      () =>
        new Promise<void>((r) => {
          resolve = r;
        }),
    );
    render(
      <LockBanner
        lock={makeLock()}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    const btn = screen.getByRole("button", { name: /take over/i });
    await act(async () => {
      btn.click();
    });
    // While the promise is pending, the button is disabled.
    expect(btn).toBeDisabled();
    await act(async () => {
      resolve();
      // Flush microtasks so the finally block runs.
      await Promise.resolve();
    });
    expect(btn).not.toBeDisabled();
  });

  it("renders 1h 4m for an age over an hour", () => {
    render(
      <LockBanner
        lock={makeLock({ ageSeconds: 3840 })}
        onTakeOver={onTakeOver}
        onCleanStale={onCleanStale}
      />,
    );
    expect(screen.getByText(/1h 4m/)).toBeInTheDocument();
  });
});
