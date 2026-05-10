# Synapseia Node UI

Desktop application for running and managing a [Synapseia](https://synapseia.network) network node. Built with **Tauri 2** (Rust backend) and **React 19** (Vite + Tailwind 4).

## Features

- **One-click node management** — Start, stop, and monitor your Synapseia node from a native desktop app.
- **Wallet panel** — View SOL and SYN balances, copy your node wallet address.
- **Staking** — Stake SYN tokens directly from the UI.
- **Real-time logs** — Stream and filter node stdout/stderr in a built-in log viewer.
- **System monitor** — CPU, memory, and disk usage at a glance.
- **Auto-updates** — OTA updates via GitHub Releases (Tauri updater plugin).
- **Cross-platform** — macOS (ARM & Intel), Windows (NSIS), Linux (AppImage).

## Architecture

```
src/                  React frontend (Vite)
  components/         UI screens and panels
    ui/               Shared design-system primitives
  hooks/              Custom React hooks (update checker, etc.)
src-tauri/            Rust backend (Tauri 2)
  src/commands.rs     IPC commands exposed to the frontend
  src/lib.rs          Plugin registration and app setup
  src/main.rs         Entry point
```

## macOS: "damaged and can't be opened"

The app is not yet code-signed with an Apple Developer ID. macOS Gatekeeper quarantines unsigned downloads and shows this error. To fix it, run this in Terminal after installing:

```bash
sudo xattr -cr "/Applications/Synapseia Node.app"
```

Then open the app normally.

## Prerequisites

| Tool | Version |
|------|---------|
| Node.js | >= 20 |
| pnpm | >= 9 |
| Rust | stable (latest) |
| Tauri CLI | `@tauri-apps/cli` v2 (installed as devDependency) |

**Platform-specific:**

- **macOS:** Xcode Command Line Tools (`xcode-select --install`)
- **Linux:** `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `patchelf`
- **Windows:** [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (bundled with Windows 10+), Visual Studio Build Tools

## Getting Started

```bash
# Install dependencies
pnpm install

# Run in development mode (hot-reload)
pnpm tauri dev

# Build production bundle
pnpm tauri build
```

## Version Policy

Synapseia Node UI, Node CLI, and Coordinator always share the **same version number**. When any component is released, all three are bumped together so node operators know exactly which versions are compatible.

## Tech Stack

- **Frontend:** React 19, Vite 7, Tailwind CSS 4, Lucide icons
- **Backend:** Rust, Tauri 2, tokio
- **Plugins:** `tauri-plugin-updater`, `tauri-plugin-os`, `tauri-plugin-process`, `tauri-plugin-shell`, `tauri-plugin-fs`, `tauri-plugin-log`

## License

Functional Source License 1.1 with Apache 2.0 future license (`FSL-1.1-ALv2`). See [`LICENSE`](./LICENSE).
