# Changelog — @synapseia/node-ui

## [2026-05-09] chore(config): drop coordinator URL knobs — env-var-only (4f7103c)

UX simplification, mirrors `@synapseia/node@0.8.2`. The desktop UI no
longer lets the user type a coordinator URL — operators that need a
non-default coordinator launch the app with `COORDINATOR_URL=...`
exported in their shell, and the spawned node CLI inherits that env
var automatically.

Frontend (React):

- `CreateNodeScreen.tsx` — removed the "Coordinator URL" input, its
  `useState`, the `http(s)://` validation, and the field from the
  `create_wallet` Tauri invoke payload.
- `SettingsPanel.tsx` — removed `coordinatorUrl` from `Config`,
  `DEFAULT_CONFIG`, the JSON parse/merge block, the
  `--set-coordinator-url` save flag, and the input + label that the
  user typed into. The Node Identity card now contains only the
  Node Name input.

Tauri (Rust):

- `commands.rs::create_wallet` — dropped the `coordinator_url: String`
  parameter, the `INVALID_COORDINATOR` validation, and the
  `--coordinator-url` arg from the spawned CLI subprocess. The child
  process inherits the parent's env by default.
- `commands.rs::read_coordinator_url` — no longer reads
  `~/.synapseia/config.json`. Returns `COORDINATOR_URL` env var or the
  hardcoded `https://api.synapseia.network` fallback. Lockstep with
  `packages/node/src/constants/coordinator.ts`.

No on-disk migration: existing `config.json` files keep their legacy
`coordinatorUrl` value, the desktop UI just ignores it.

Versions: `package.json`, `src-tauri/Cargo.toml`,
`src-tauri/tauri.conf.json` all bumped `0.8.3` → `0.8.4` per the
version-sync rule.

## [2026-05-07] chore(release): version sync 0.6.0/0.8.0 → 0.8.1

Beta-launch slice S6. Synced version across all three sources of
truth (sub `feedback_version_sync.md` rule):

- `package.json`: `0.8.0` → `0.8.1`.
- `src-tauri/Cargo.toml`: `0.6.0` → `0.8.1` (was drifted from prior
  Sprint 11+12 bump).
- `src-tauri/tauri.conf.json`: `0.6.0` → `0.8.1` (same drift).

Aligns node-ui with `@synapseia/coordinator@0.8.1` and
`@synapseia/node@0.8.1` (S5). Tagging `node-ui-v0.8.1` in this sub-repo
triggers `.github/workflows/release-node-ui.yml` 4-job matrix
(macOS arm64 + macOS x64 + Linux + Windows) and publishes the GitHub
Release with `.dmg`, `.msi`, `.AppImage` artifacts.

Tag push is operator-driven — see commit message for the exact
command.

## [2026-05-07] feat(beta): pre-flight capacity probe + BetaLimitModal

Closed devnet beta launch — slice S3.

**Rust side**

- `src-tauri/Cargo.toml`: add `reqwest` (rustls-tls + json, no
  default features) for the lightweight capacity probe.
- `src-tauri/src/commands.rs`: new `check_capacity` Tauri command.
  Reads `coordinatorUrl` from `~/.synapseia/config.json`, hits
  `GET <coord>/peer/capacity` (5 s timeout), returns
  `{ limit, current, accepting }`. Network/HTTP errors bubble up
  as `Err` so the frontend can fall through to the existing
  `start_node` path instead of showing a false-positive modal.
- `src-tauri/src/lib.rs`: register `check_capacity` in the
  `invoke_handler!` macro.

**React side**

- `src/components/BetaLimitModal.tsx` (new): full-screen overlay
  mirroring `ActivationScreen` (Tailwind only, `Card` + `Button`
  primitives, `lucide-react` icon). Backdrop click / ESC / Enter /
  OK button all close. OK button autofocused. Renders
  `{current} / {limit} nodes registered` only when both numbers
  are known (zero on the fallback path).
- `src/App.tsx`: `handleStartNode` pre-flights the capacity probe.
  When `accepting === false` it shows the modal and never spawns
  the CLI. The `node-log` listener also matches
  `/^\[BETA_LIMIT_REACHED\]/` against the message field —
  catches the race where the cap fills between the probe and the
  CLI's first heartbeat (CLI emits the marker per slice S2). Modal
  is rendered both on the unlocked dashboard and on the activation
  screen so a marker firing during the post-activation auto-start
  is still visible.

## [2026-05-03] feat(node-ui): provider+tier dropdowns, drop Custom… and LLM URL field (cf307c2)

Replaced the flat `POPULAR_MODELS` list with two coupled dropdowns:
**Provider** (OpenAI, Anthropic, Google, Kimi, MiniMax, Zhipu, Ollama)
and **Tier** (Top / Mid / Budget) — three models per cloud provider.
The `Custom…` option and the free-form `LLM API URL` input are gone:
every cloud endpoint is hardcoded by the node so the wire-protocol
adapter can rely on a known response shape.

The provider list lives in `src/lib/providers.ts` (mirror of the
node's authoritative table). Both `SettingsPanel.tsx` and
`CreateNodeScreen.tsx` consume it. The Tauri IPC `create_wallet` call
still receives an `llmUrl` parameter for one release (set to `null`)
so older Rust shells keep working — the node CLI logs a deprecation
WARN and ignores the value.

API-key field tooltip now lists the env var the operator can set per
provider (OPENAI_API_KEY, ANTHROPIC_API_KEY, GEMINI_API_KEY,
MOONSHOT_API_KEY, MINIMAX_API_KEY, ZHIPU_API_KEY) instead of asking
for a generic OpenAI-style endpoint URL.

## [2026-05-02] release: v1.0.0 — public-network milestone

First stable release of the Synapseia Node desktop UI (Tauri 2 + React).

What "1.0.0" means in this codebase:

- **Tauri 2 shell stabilised** — capability/permission map locked to
  the minimum required for node operation (Solana wallet, file
  access for model cache, local node IPC). No broad APIs.
- **Bundled Synapseia node runtime** — the desktop app drives the
  same `@synapseia/node` 1.0.0 binary, so capabilities and protocol
  match the headless release.
- **Wallet panel + activation flow** — unlock → activation →
  stake/tier visualisation hardened (see 2026-04-23 fixes).
- **Dark theme, Liquid Glass-inspired surfaces** consistent with the
  dashboard.

Limits acknowledged for 1.0.0:

- macOS + Linux signed builds are the reference target. Windows
  builds compile but are unsigned for now.
- Auto-update relies on the GitHub release feed of the node
  sub-repo; in-app rollback is post-1.x.

Version sync: matches `@synapseia/coordinator` 1.0.0 and
`@synapseia/node` 1.0.0. Cargo crate `synapseia-node-ui` and the
Tauri config are pinned to the same `1.0.0`.

## [2026-04-29] feat(docking): Tauri capability check + reward type label (cb6a6b0)

Layer 1 task 6/12 of the 4-layer pharma plan
(`~/.claude/plans/lucky-mixing-dongarra.md`) — desktop UI surface for
the MOLECULAR_DOCKING workload.

- New Tauri `docking_capabilities()` async command. Probes for
  `vina` + `obabel` on the augmented PATH (re-uses the existing
  `which_in_path` helper) and runs each with `--version`/`-V` to
  report the resolved version. Returns
  `{ vina_available, vina_path, vina_version, obabel_available,
  obabel_path, obabel_version }`. Frontend can call it once on
  mount to surface a banner / disabled-toggle for users whose
  machines lack the docking binaries — non-blocking, every other
  workload keeps working.
- New `docking` entry in `MyNodePanel`'s `TYPE_LABELS`
  (fuchsia accent) so the rewards-by-type breakdown renders
  `RewardType.DOCKING` rows correctly.

`cargo check` clean. `vite build` clean.

## [2026-04-27] feat(logs-panel): stretch logs box to fill the full vertical viewport (89eb0ac)

`LogViewer` used a static `h-[calc(100vh-200px)]` for the log box, leaving
unused vertical space below it. Switched the panel to flex-col with
`flex-1 min-h-0` on the scroll region so the box always fills whatever
height the parent `<main>` exposes, regardless of viewport size or
header changes. Header row gets `shrink-0` so it doesn't compete for
space.

## [2026-04-27] fix(tauri): inject AGENT_BRAIN_PATH=<appDataDir>/agent-brain.json on start_node (1fb0484)

Tauri spawns the node child with `cwd='/'`. Without an explicit
`AGENT_BRAIN_PATH` env, the node had to guess a writable location relative
to its install dir. Pinning the brain to Tauri's per-OS app data dir is
the canonical fix; the node-side `__dirname`-based fallback (node/4f27eaff)
is the safety net for non-Tauri spawns.

Resolves the runtime error `Failed to save brain to /data/agent-brain.json:
ENOENT: no such file or directory, mkdir '/data'`.

- macOS:   `~/Library/Application Support/network.synapseia.node-ui/`
- Linux:   `~/.local/share/network.synapseia.node-ui/`
- Windows: `%APPDATA%\network.synapseia.node-ui\`

Adds `tauri::Manager` to the imports so `app.path().app_data_dir()`
resolves. Logs a stderr warning and lets the node fall back to its
moduleDir-relative resolver if `create_dir_all` fails.

## [2026-04-26] chore(node-ui): regenerate Cargo.lock after 0.4.0 version bump (e4e4e92)

Lockfile didn't update during the 0.4.0 release commit; checked in to keep
Tauri/Cargo state consistent with `src-tauri/tauri.conf.json`.

## [0.4.0] 2026-04-26 — version sync release

- Bumped version to 0.4.0 (synced with coordinator and node).
- Renamed package from `tauri-app` to `@synapseia/node-ui`.
- Updated Tauri updater endpoint to `erscoder/synapseia-node-ui`.
- Updated Cargo.toml version to 0.4.0.
- Added comprehensive README.

## [0.2.0] 2026-04-25 — feat(version-gating): T7 Tauri auto-updater

- Configured Tauri updater plugin: endpoints point to GitHub Releases
  `latest.json`. Pubkey placeholder (needs `cargo tauri signer generate`).
- New Rust command `check_for_updates`: uses `tauri-plugin-updater` to
  query GitHub Releases for new versions.
- New `useUpdateChecker` React hook: polls every 30 min, exposes
  `installUpdate()` (download + relaunch) and `dismiss()`.
- New `UpdateBanner.tsx`: emerald banner at top of dashboard, user-initiated
  (click "Update & Restart"), not forced.
- Added `@tauri-apps/plugin-process` for `relaunch()`.
- Version bumped: package.json 0.2.0, tauri.conf.json 0.2.0, Cargo.toml 0.2.0.

## [2026-04-24] feat(node-ui): initial git import + security audit (493c6a6)

Stack tracked: Tauri 2.10 + React 19.2 + Vite 7 + Tailwind 4.
Full source + icons + tauri.conf committed to sub-repo.

**osv-scanner audit at import (packages/node-ui/):**

| Ecosystem | Findings | Fixable | Upstream-blocked |
|---|---|---|---|
| pnpm (JS) | 0 | 0 | 0 |
| cargo (Rust) | 19 | 0 effective | 19 |

- 17 "unmaintained" warnings: gtk-rs GTK3 bindings (atk, gdk, gtk,
  gdkx11, gdkwayland-sys, gtk-sys, gtk3-macros, atk-sys, pango…),
  unic-* (unic-char-property, unic-char-range, unic-common,
  unic-ucd-ident, unic-ucd-version), proc-macro-error, fxhash.
- 2 "unsoundness" advisories with a nominal fix:
  - `glib 0.18.5 → 0.20.0` (RUSTSEC-2024-0429, CVSS 6.9) — arrives via
    `webkit2gtk → wry → tauri-runtime-wry → tauri`. Blocked: Tauri 2.x
    locks glib 0.18 until its GTK4/webkit6 migration lands upstream.
  - `rand 0.7.3 → 0.8.6` (RUSTSEC-2026-0097) — arrives via
    `phf_generator 0.8 → phf_codegen → selectors → kuchikiki →
    tauri-utils`. Build-script only (compile-time), no runtime impact.

All 19 findings are transitive deps owned by Tauri 2. Will clear when
Tauri ships its GTK4/webkit6 migration (tracking upstream).

## [2026-04-24] fix(shutdown): reliable node kill on app close

- Replace `try_lock()` with `blocking_lock()` in `reap_on_exit` so the node
  child is always reaped even when a background task (log streaming, status
  poll) holds the mutex at the moment the window closes.
- Add SIGTERM → 2 s grace → SIGKILL sequence on Unix so the Node.js process
  can flush logs and close WebSocket connections before being force-killed.
- `cleanup_lock_if_ours` now runs on the clean-exit path too, preventing a
  stale `~/.synapseia/node.lock` after graceful shutdown.
