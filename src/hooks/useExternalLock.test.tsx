import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act, waitFor } from "@testing-library/react";

// Hoisted mock so the module-level `import { invoke }` in the hook picks it up.
const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

import { useExternalLock, type ExternalLockInfo } from "./useExternalLock";

const aliveLock: ExternalLockInfo = {
  pid: 16451,
  source: "cli",
  startedAt: "2026-05-17T10:00:00.000Z",
  ageSeconds: 60,
  isAlive: true,
};

// Wait for `invokeMock` to have been called `n` times. Polls at 10ms so
// short test polls (we use 50ms) settle deterministically without burning a
// fake-timer abstraction (which deadlocks against testing-library's own
// `waitFor` setTimeout).
async function waitForCallCount(n: number, timeoutMs = 2000) {
  await waitFor(
    () => {
      expect(invokeMock).toHaveBeenCalledTimes(n);
    },
    { timeout: timeoutMs, interval: 10 },
  );
}

describe("useExternalLock", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("returns null when there is no lock", async () => {
    invokeMock.mockResolvedValue(null);
    const { result } = renderHook(() => useExternalLock(60_000));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("check_external_lock");
    });
    expect(result.current.lock).toBeNull();
  });

  it("returns the lock info when present", async () => {
    invokeMock.mockResolvedValue(aliveLock);
    const { result } = renderHook(() => useExternalLock(60_000));
    await waitFor(() => {
      expect(result.current.lock).toEqual(aliveLock);
    });
  });

  it("polls at the configured interval", async () => {
    invokeMock.mockResolvedValue(null);
    renderHook(() => useExternalLock(50));
    // Initial fetch + at least one interval-driven poll within 500 ms.
    await waitForCallCount(1);
    await waitForCallCount(2);
    await waitForCallCount(3);
  });

  it("reflects state transition null → lock → null across polls", async () => {
    invokeMock
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(aliveLock)
      .mockResolvedValueOnce(null)
      .mockResolvedValue(null);
    const { result } = renderHook(() => useExternalLock(50));
    await waitFor(() => expect(result.current.lock).toBeNull());
    await waitFor(() => expect(result.current.lock).toEqual(aliveLock));
    await waitFor(() => expect(result.current.lock).toBeNull());
  });

  it("stops polling on unmount", async () => {
    invokeMock.mockResolvedValue(null);
    const { unmount } = renderHook(() => useExternalLock(50));
    await waitForCallCount(2);
    unmount();
    const callsAtUnmount = invokeMock.mock.calls.length;
    // Wait > 3 intervals. If polling kept running, the count would grow.
    await new Promise((r) => setTimeout(r, 250));
    expect(invokeMock.mock.calls.length).toBe(callsAtUnmount);
  });

  it("treats invoke rejection as no-lock (drops the banner instead of stale data)", async () => {
    invokeMock.mockRejectedValue(new Error("ipc dropped"));
    const { result } = renderHook(() => useExternalLock(60_000));
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalled();
    });
    expect(result.current.lock).toBeNull();
  });

  it("exposes a refresh() that triggers an immediate poll", async () => {
    invokeMock.mockResolvedValue(null);
    const { result } = renderHook(() => useExternalLock(60_000));
    await waitForCallCount(1);
    invokeMock.mockResolvedValueOnce(aliveLock);
    await act(async () => {
      await result.current.refresh();
    });
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(result.current.lock).toEqual(aliveLock);
  });
});
