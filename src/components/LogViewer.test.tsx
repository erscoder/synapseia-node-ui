import { describe, expect, it, vi } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { LogViewer } from "./LogViewer";
import { NodeStatus } from "../App";

function makeStatus(running: boolean): NodeStatus {
  return {
    running,
    peer_id: null,
    tier: null,
    wallet: null,
    balance_sol: null,
    balance_syn: null,
    staked_syn: null,
    pid: running ? 1234 : null,
  };
}

describe("LogViewer Start/Stop control", () => {
  it("renders Start when node is stopped and password is set; clicking calls onStart", () => {
    const onStart = vi.fn();
    const onStop = vi.fn();
    render(
      <LogViewer
        logs={[]}
        status={makeStatus(false)}
        password="hunter2"
        onStart={onStart}
        onStop={onStop}
      />,
    );

    const startBtn = screen.getByRole("button", { name: /start node/i });
    expect(startBtn).not.toBeDisabled();
    expect(screen.queryByRole("button", { name: /stop node/i })).toBeNull();

    act(() => {
      startBtn.click();
    });

    expect(onStart).toHaveBeenCalledTimes(1);
    expect(onStop).not.toHaveBeenCalled();
  });

  it("disables Start when password is null (wallet locked)", () => {
    render(
      <LogViewer
        logs={[]}
        status={makeStatus(false)}
        password={null}
        onStart={vi.fn()}
        onStop={vi.fn()}
      />,
    );
    expect(screen.getByRole("button", { name: /start node/i })).toBeDisabled();
  });

  it("renders Stop when node is running; clicking calls onStop", () => {
    const onStart = vi.fn();
    const onStop = vi.fn();
    render(
      <LogViewer
        logs={[]}
        status={makeStatus(true)}
        password="hunter2"
        onStart={onStart}
        onStop={onStop}
      />,
    );

    const stopBtn = screen.getByRole("button", { name: /stop node/i });
    expect(screen.queryByRole("button", { name: /start node/i })).toBeNull();

    act(() => {
      stopBtn.click();
    });

    expect(onStop).toHaveBeenCalledTimes(1);
    expect(onStart).not.toHaveBeenCalled();
  });

  it("always renders the Export button regardless of run state", () => {
    const { rerender } = render(
      <LogViewer
        logs={[]}
        status={makeStatus(false)}
        password="hunter2"
        onStart={vi.fn()}
        onStop={vi.fn()}
      />,
    );
    expect(screen.getByRole("button", { name: /export/i })).toBeInTheDocument();

    rerender(
      <LogViewer
        logs={[]}
        status={makeStatus(true)}
        password="hunter2"
        onStart={vi.fn()}
        onStop={vi.fn()}
      />,
    );
    expect(screen.getByRole("button", { name: /export/i })).toBeInTheDocument();
  });
});
