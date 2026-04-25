import { Download, X } from "lucide-react";

interface UpdateBannerProps {
  version: string | null;
  installing: boolean;
  error: string | null;
  onInstall: () => void;
  onDismiss: () => void;
}

export function UpdateBanner({
  version,
  installing,
  error,
  onInstall,
  onDismiss,
}: UpdateBannerProps) {
  return (
    <div className="flex items-center gap-3 bg-emerald-900/40 border border-emerald-700/50 rounded-lg px-4 py-2 text-sm text-emerald-200">
      <Download className="w-4 h-4 shrink-0" />
      <span className="flex-1">
        {installing
          ? "Installing update..."
          : error
            ? `Update failed: ${error}`
            : `Version ${version ?? "?"} is available.`}
      </span>
      {!installing && (
        <>
          <button
            onClick={onInstall}
            className="px-3 py-1 rounded bg-emerald-600 hover:bg-emerald-500 text-white text-xs font-medium transition-colors"
          >
            Update & Restart
          </button>
          <button
            onClick={onDismiss}
            className="p-1 rounded hover:bg-emerald-800/50 transition-colors"
            title="Dismiss"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </>
      )}
    </div>
  );
}
