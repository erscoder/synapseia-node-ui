import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input } from "./ui";

interface UnlockResult {
  success: boolean;
  wallet_address: string | null;
  error_code: string | null;
  error_message: string | null;
}

interface Props {
  onUnlock: (password: string, walletAddress: string) => void;
}

export function UnlockScreen({ onUnlock }: Props) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const handleUnlock = async () => {
    setError(null);
    if (!password) {
      setError("Please enter your password.");
      return;
    }

    setLoading(true);
    try {
      const result = await invoke<UnlockResult>("unlock_wallet", { password });

      if (!result.success) {
        setError(result.error_message ?? "Incorrect password.");
        setLoading(false);
        return;
      }
      if (!result.wallet_address) {
        setError("Unlock returned no wallet address. Please try again.");
        setLoading(false);
        return;
      }

      // Keep `loading` true — onUnlock triggers parent to unmount this screen.
      onUnlock(password, result.wallet_address);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setLoading(false);
    }
  };

  return (
    <div className="flex flex-col items-center justify-center min-h-screen text-slate-100">
      <div className="w-full max-w-md p-8 space-y-6">
        <div className="text-center space-y-2">
          <div className="w-20 h-20 mx-auto flex items-center justify-center">
            <img
              src="/synapseia-logo.png"
              alt="Synapseia"
              className="w-full h-full object-contain drop-shadow-[0_0_40px_rgba(0,212,255,0.25)]"
            />
          </div>
          <h1 className="text-3xl font-bold text-slate-100 tracking-tight">Synapseia Node</h1>
          <p className="text-slate-400 text-sm">Unlock your node to begin</p>
        </div>

        <Card padding="md" className="space-y-4">
          <Input
            type="password"
            label="Vault passphrase"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !loading && handleUnlock()}
            placeholder="Enter the 12+ character vault passphrase"
            disabled={loading}
            autoFocus
          />

          {error && (
            <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-3">
              <p className="text-sm text-red-300 break-words">{error}</p>
            </div>
          )}

          <Button
            variant="primary"
            size="lg"
            onClick={handleUnlock}
            disabled={loading || !password}
            className="w-full"
          >
            {loading ? "Unlocking…" : "Unlock Node"}
          </Button>
        </Card>

        <p className="text-xs text-slate-500 text-center">
          The vault passphrase is the 12+ character one you set the first time you ran the node. It never leaves this machine — it decrypts the local wallet keystore.
        </p>
      </div>
    </div>
  );
}
