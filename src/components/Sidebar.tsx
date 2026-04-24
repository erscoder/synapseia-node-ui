import { Panel } from "../App";
import { clsx } from "clsx";
import {
  Fingerprint,
  Wallet,
  TrendingUp,
  Server,
  Settings,
  Terminal,
} from "lucide-react";
import { StatusPill } from "./ui";

interface Props {
  activePanel: Panel;
  onPanelChange: (panel: Panel) => void;
  nodeRunning: boolean;
}

const navItems: { panel: Panel; label: string; icon: typeof Fingerprint }[] = [
  { panel: "my-node", label: "My Node", icon: Fingerprint },
  { panel: "wallet", label: "Wallet", icon: Wallet },
  { panel: "stake", label: "Stake", icon: TrendingUp },
  { panel: "system", label: "System", icon: Server },
  { panel: "settings", label: "Settings", icon: Settings },
  { panel: "logs", label: "Logs", icon: Terminal },
];

export function Sidebar({ activePanel, onPanelChange, nodeRunning }: Props) {
  return (
    <aside className="w-56 bg-[var(--bg-surface)]/60 backdrop-blur-md border-r border-white/[0.06] flex flex-col">
      <div className="p-4 border-b border-white/[0.06]">
        <div className="flex items-center gap-2">
          <div className="w-8 h-8 flex items-center justify-center">
            <img
              src="/synapseia-logo.png"
              alt="Synapseia"
              className="w-full h-full object-contain drop-shadow-[0_0_12px_rgba(0,212,255,0.35)]"
            />
          </div>
          <span className="font-bold text-slate-100 tracking-tight">Synapseia</span>
        </div>
      </div>

      <nav className="flex-1 p-3 space-y-1">
        {navItems.map(({ panel, label, icon: Icon }) => (
          <button
            key={panel}
            onClick={() => onPanelChange(panel)}
            className={clsx(
              "w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-sm font-medium transition-all",
              activePanel === panel
                ? "bg-white/[0.06] text-slate-100 border border-white/[0.1]"
                : "text-slate-400 hover:text-slate-100 hover:bg-white/[0.03] border border-transparent"
            )}
          >
            <Icon className="w-4 h-4" />
            {label}
          </button>
        ))}
      </nav>

      <div className="p-3 border-t border-white/[0.06]">
        <StatusPill tone={nodeRunning ? "green" : "slate"} pulse={nodeRunning} className="w-full justify-center">
          {nodeRunning ? "Node Online" : "Node Offline"}
        </StatusPill>
      </div>
    </aside>
  );
}
