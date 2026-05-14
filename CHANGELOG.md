# Changelog — @synapseia/node-ui

## [2026-05-14] chore(release): 0.8.37 lockstep bump for node multi-slash slug fix (e8f6a8b)

Version-only bump. Node-ui has no functional change in this cycle.
Node 0.8.37 fixes the NVIDIA NIM persistence bug — CLI
`--set-model` regex now accepts multi-slash modelIds like
`nvidia/meta/llama-3.3-70b-instruct` and
`nvidia/nvidia/nemotron-3-super-120b-a12b`. The Settings panel
no longer surfaces "Failed to apply CLI config update --set-model"
when the operator picks NVIDIA NIM tiers; provider/model/API key
now persist correctly end-to-end. Lockstep keeps coord + node +
node-ui versioned together.


## [2026-05-14] fix(settings): persist cloud LLM config end-to-end (83abca2)

Production bug: Windows operators selecting NVIDIA NIM (or any cloud
provider) + model + API key in Settings would silently revert to
default Ollama after ~3 seconds, then the node would crash on next
start with hasCloudLlm=false because no LLM_CLOUD_* env vars reached
the CLI subprocess.

Root cause: UiSettings struct only persisted ollama_url; the CLI
spawn only forwarded OLLAMA_URL; SettingsPanel did not check
run_command.success so a Windows-side CLI failure passed silently
and loadConfig() then reverted the UI from the unchanged CLI
config file.

Fix wires the Tauri-side ui-settings.json as source of truth and
threads cloud env vars into every CLI spawn:

- UiSettings struct gains llm_provider, llm_model_slug, llm_api_key
  with serde defaults so legacy ui-settings.json files still load.
- set_ui_settings accepts 4 args (Option<String> merge semantics).
- File perms 0o600 on Unix; Windows inherits %USERPROFILE% ACL.
- build_node_command injects LLM_PROVIDER=cloud + LLM_CLOUD_PROVIDER
  + LLM_CLOUD_MODEL + <PROVIDER>_API_KEY for the 7 cloud providers
  on every CLI spawn when a non-ollama provider is selected.
- SettingsPanel surfaces run_command failure as a real error and
  skips loadConfig() so a failed save no longer silently reverts.
- CreateNodeScreen mirrors the initial cloud selection to
  ui-settings AFTER create_wallet succeeds so the first
  synapseia start already has the env vars.

9 Rust tests + 2 vitest tests covering serde compat, env injection,
failure-surface flow, and the full 4-arg invoke shape.

Version bump 0.8.35 -> 0.8.36 lockstep.

## [2026-05-14] chore(release): 0.8.35 lockstep bump for node hardware hang hotfix (e63c9a0)

Version-only bump. node-ui has no functional change in this cycle.
Bundles node 0.8.35 (Windows nvidia-smi hang fix) via the standard
install-on-demand path; users who already have the desktop app see
the new node CLI auto-installed the next time they start their node.

## [2026-05-14] feat(ui): NVIDIA NIM provider + free-tier registration hint (9230c6c)

NVIDIA NIM joins the cloud-LLM provider dropdown. Picking it surfaces
an emerald hint with a direct link to build.nvidia.com so operators
without a paid LLM subscription or local GPU can run a node at zero
cost using their personal NGC free-tier key (~5,000 credits/month).

- `providers.ts` UI mirror gains the NVIDIA entry. Tiers match the
  authoritative table in `@synapseia-network/node`:
    top:    nvidia/nemotron-3-super-120b-a12b (120B MoE flagship).
    mid:    meta/llama-3.3-70b-instruct (production stable).
    budget: meta/llama-3.2-3b-instruct (fast & light).
- `CreateNodeScreen.tsx`: conditional emerald hint rendered below the
  API-key field whenever NVIDIA is selected. Links to build.nvidia.com.
- `SettingsPanel.tsx`: same hint, rendered when NVIDIA is the active
  provider and no key is currently stored.

Version bump 0.8.33 -> 0.8.34 lockstep with coord + node + landing.

## [2026-05-12] ui(my-node-panel): mirror dashboard Rewards card refactor (5141c77)

Brings the desktop app's `MyNodePanel` Rewards breakdown to behavior
parity with the web dashboard `/my-node` page shipped earlier this
cycle.

Aggregated 5-group breakdown (Research / Training (training + diloco
+ lora + docking) / Inference (cpu_inference + gpu_inference,
purple-400 new) / Peer Review / Work Orders). Zero-sum groups hidden.
`EXCLUDED_KEYS = {staking, referral}` so staking rewards aren't
double-counted (operators manage staking from
`app.synapseia.network/staking`; the desktop app intentionally has
no staking surface) and the never-emitted `referral` type doesn't
render a phantom row. Unknown future RewardType keys fall through to
a slate-400 fallback row.

Node-ui-specific deviations from the dashboard pattern: keeps the
`!hasAnyRow && claimable === 0` empty-state condition (original
desktop UX for fresh nodes) and the `recentlyClaimed` optimistic-
clear flow.

Reviewer SHIP after fixing a P10 JSDoc lie — original draft said
staking lives "in the Staking section of the dashboard" which is
false from the desktop app's POV; rewrite points operators at the
web dashboard URL.

File: `src/components/MyNodePanel.tsx` (~+40 LOC net).

## [2026-05-11] chore(version): align node-ui to 0.8.17 + Linux AppImage fix (765bfef)

Lockstep bump. Pairs with `fix(ci): install libfuse2 for AppImage
bundling` (9419ed1). The 0.8.16 ubuntu build failed at the
`linuxdeploy` step because the GitHub ubuntu-22.04 runner stopped
pre-installing libfuse2 (linuxdeploy is itself an AppImage and
needs FUSE2 to self-mount). 0.8.17 should ship the AppImage too.

## [2026-05-11] chore(version): align node-ui to 0.8.16 with coord + node (29fc07e)

Lockstep with `node 0.8.16` (pins `@libp2p/utils@7.1.0` so the
postinstall `patch-package` step succeeds on fresh installs).
Node-ui code unchanged from 0.8.15; bumping to keep CI's
wait-for-npm + bundle pins targeting the patched CLI version.

## [2026-05-11] fix(bundle): apply patch-package from scratch root (3cc9ea5)

First node-ui-v0.8.15 release run (`25678828203`) failed on the new
CI bundle step:

```
Error: Patch file found for package utils which is not present at
node_modules/@libp2p/utils
```

Root cause: `npm install --omit=dev @synapseia-network/node` hoists
`@libp2p/utils` to the scratch root's `node_modules/`, but the
postinstall `patch-package` runs with cwd at
`scratch/node_modules/@synapseia-network/node/` and looks for
`./node_modules/@libp2p/utils` nested — which doesn't exist.

Fix: install with `--ignore-scripts` so the postinstall is skipped,
then run `patch-package@8` manually from the scratch root pointing
`--patch-dir` at the installed package's `patches/` subdir.

Applied to both `release.yml` (CI per-platform build) and
`scripts/bundle-cli.mjs::bundleFromNpm()` (local materializer).
The `bundleFromWorkspace` mode is unaffected.

## [2026-05-11] chore(version): align node-ui to 0.8.15 with coord + node (4b564d9)

Lockstep bump. Node-ui code changes in this version cycle:
- CLI bundling inside the bundle as last-resort fallback (1c23393)

Tag `node-ui-v0.8.15` triggers `release.yml`, which:
1. Waits up to 10 min for `@synapseia-network/node@0.8.15` on the npm
   registry.
2. Runs `npm install --omit=dev @synapseia-network/node@0.8.15` on
   each platform matrix runner (so the platform-specific `usearch`
   native binding is correct).
3. Copies the installed CLI into `src-tauri/resources/cli/` before
   `tauri-action` builds the DMG/MSI/AppImage.

This guarantees the bundled CLI version inside the desktop bundle
matches the desktop app version 1:1.

## [2026-05-11] feat(bundle): ship CLI inside the .dmg/.msi/.AppImage (1c23393)

Closes the recurring "ERR_CLI_MISSING on first launch" failure mode:
new users without a global Node/npm install (or with permission /
network issues on `npm install -g`) now get a working node CLI from
inside the bundle, zero network required.

Priority order in `find_synapseia_node` (unchanged for existing
users): dev path → homebrew/global npm roots →
`~/.synapseia/node/lib/node_modules/...` → `npm root -g` dynamic →
**bundled resource (NEW, last)**. CLI auto-update via
`npm install -g` continues to take precedence over the bundle.

Implementation:
- `src-tauri/tauri.conf.json` declares
  `resources/cli/{dist,node_modules,patches,package.json}` in
  `bundle.resources`.
- `src-tauri/src/commands.rs`: `find_synapseia_node` accepts
  `Option<&AppHandle>` and resolves `app.path().resource_dir() / "cli"`.
  `build_node_command(app, args)` threads the handle through to all
  Tauri commands (`start_node`, `fetch_chain_info`, `unlock_wallet`,
  `create_wallet`, `run_command`). Substring check on bundled
  `package.json` plus existence check on `dist/index.js` — defense-
  in-depth (placeholder package name differs from the runtime CLI
  name).
- `.github/workflows/release.yml`: per-matrix-runner step now
  - waits up to 10 min for `@synapseia-network/node@$VERSION` to
    appear on npm (avoids shipping a stale CLI when node-ui-v* and
    node-v* tags race),
  - then `npm install --omit=dev @synapseia-network/node@$VERSION`
    into a scratch dir on the same platform runner (picks up
    platform-specific `usearch` native binding), and
  - copies into `src-tauri/resources/cli/` before `tauri-action`.
- `scripts/bundle-cli.mjs`: local materializer with workspace and
  npm modes (version self-pinned from own `package.json`).
- `package.json`: new `bundle:cli` script, chained into `build:dmg`.
- `src-tauri/resources/cli/package.json` placeholder uses
  `@synapseia-network/node-bundle-placeholder` so an empty scaffold
  can't satisfy the runtime substring check.
- `.gitignore`: scaffold tracked (`.gitkeep` + placeholder
  `package.json`), real bundled files ignored.

Bundle size impact: ~150–250 MB per platform. CI prints `du -sh`
each run.

Reviewer pass: 1 BLOCKER (version pin), 1 HIGH (placeholder name)
addressed; MEDIUMs (npm cache, progress message clarity) deferred.

## [2026-05-11] chore(version): align node-ui to 0.8.13 with node + coord (44040d7)

Lockstep with node 0.8.13. UI code unchanged.

## [2026-05-11] chore(version): align node-ui to 0.8.12 with node + coord (0b151e0)

Lockstep with node 0.8.12 (WeakMap iteration on the
`@libp2p/utils` `onProgress` patch). UI code unchanged.

## [2026-05-11] chore(version): align node-ui to 0.8.11 with node + coord (a165fac)

Lockstep with node 0.8.11 (libp2p/utils onProgress guard). UI
code unchanged; bumped `package.json`, `Cargo.toml`, and
`tauri.conf.json` so the release CI tag publishes binaries with
matching version metadata.

## [2026-05-10] fix(security): SHA256 verify Node tarball + serialize runtime + macOS quarantine (ae6db36)

Three fixes from the 0.8.9 reviewer pass on `ensure_node_runtime`:

- **BLOCKER (SHA256 verify)**: every Node tarball downloaded from
  `nodejs.org/dist/` is now sha256-checked against the official
  `SHASUMS256.txt` BEFORE extraction. Mismatch or manifest fetch
  failure hard-fails the install. Closes the MITM surface where
  a hostile network or DNS hijack could swap the tarball.
  Adds `sha2 = "0.10"` to Cargo.toml.
- **HIGH (concurrent runtime download)**: new
  `NODE_RUNTIME_LOCK` static `tokio::sync::Mutex` acquired at
  the very start of `ensure_node_runtime`. Coexists with
  `INSTALL_LOCK` — separate locks (Option A) avoid the
  non-reentrant `tokio::sync::Mutex` deadlock that a single
  combined lock would have introduced.
- **HIGH (macOS Gatekeeper)**: best-effort
  `xattr -dr com.apple.quarantine` on the extracted runtime
  tree (gated `#[cfg(target_os = "macos")]`). Defends against
  binaries refusing to launch under stricter Gatekeeper
  policies.

Bonus mediums folded in: `ArchiveCleanup` RAII guard ensures the
tarball is deleted on every exit path (no more 30 MB leak in
`/tmp` on extract failure), and the staging directory moved to
`~/.synapseia/node-staging-vX.Y.Z/` so the final
`rename(inner, target_root)` is intra-fs and atomic on
cross-device setups (Linux tmpfs `/tmp` vs ext4 `/home`).

Version bumped to 0.8.10 (sync with coord + node).

## [2026-05-10] feat(autoinstall): auto-download Node.js v22 LTS when system has none (df76734)

Closes the last hard-fail in the install chain: the desktop app
no longer requires the user to have Node.js pre-installed. When
`locate_node_binary()` returns None and no bundled runtime is
present yet at `~/.synapseia/node/`, the new `ensure_node_runtime`
helper downloads the official Node v22 LTS tarball from
nodejs.org/dist for the host platform/arch (darwin-arm64,
darwin-x64, linux-x64, linux-arm64, win-x64), shells out to
`tar -xf` (xz/gz/zip auto-detect on macOS/Linux/Windows 10+) into
a staging dir, strips the version-prefixed top-level dir, and
plants `~/.synapseia/node/bin/node` + `bin/npm` (or `node.exe` +
`npm.cmd` on Windows). The subsequent
`npm install -g @synapseia-network/node` runs against that
bundled toolchain. Node version pinned via `BUNDLED_NODE_VERSION`
constant — bump deliberately when LTS rolls forward.

`find_synapseia_node()` gained a probe for
`~/.synapseia/node/lib/node_modules/@synapseia-network/node` so
post-install lookups against the bundled runtime succeed.

New install-progress phases: `downloading-node` and `node-ready`.
Frontend already pipes the events through to the existing
spinner copy, no UI changes needed. Total first-boot install on
a Node-less machine is ~30 MB download + ~5 s extract + the
~5 s npm install — under a minute on broadband.

Unsupported platforms (FreeBSD, OpenBSD, 32-bit, ARM Windows,
RISC-V) still return the manual-install message.

Version bumped to 0.8.9.

## [2026-05-10] fix(autoinstall): legacy bin collision + non-fatal boot install (2a5307b)

Two follow-ups to the 0.8.7 boot-time auto-install:

- **Bin collision (EEXIST)**: users who had the legacy
  `@synapseia/node` (pre-rename) installed globally hit
  `EEXIST: file already exists - .../bin/syn` because the new
  package declares `synapseia` + `syn` bins on top of the old
  package's bins. The Tauri command now does a best-effort
  `npm uninstall -g @synapseia/node` first (errors ignored — the
  most common case is "package not installed") and passes
  `--force` to `npm install -g @synapseia-network/node` so an
  EEXIST on the bin overlap doesn't strand the install.
- **Non-fatal boot install**: when `install_synapseia_node` errored
  during the boot useEffect the `return` short-circuited
  `wallet_exists`, leaving the user stuck on "Checking wallet…"
  with the install error invisible. Drop the early return — the
  wallet check now runs unconditionally and the
  `handleStartNode` ERR_CLI_MISSING fallback covers the retry on
  Start.

Version bumped to 0.8.8 (sync with coord + node).

## [2026-05-10] fix(autoinstall): run install_synapseia_node at boot, not just on Start (fad89a8)

Existing-wallet users were hitting `ERR_CLI_MISSING: Could not
locate @synapseia-network/node` after upgrading from a pre-rename
build because the auto-install fallback only fired inside
`handleStartNode`. If the user never reached the Start button (or
the CLI was wiped between sessions) the desktop app would surface
the locate error in the LogViewer instead of installing.

Fix: chain `install_synapseia_node` -> `wallet_exists` in the boot
useEffect so EVERY launch verifies the CLI is on disk before
deciding create-vs-unlock. The Tauri command is idempotent — early
return when the package is already installed, so the common path
adds <100 ms. The `handleStartNode` fallback stays as a secondary
safety net for cases where the CLI vanishes between unlock and
start. Dismiss button on the install-error screen now re-routes
through `wallet_exists` instead of forcing `unlocked` (the wallet
might not be unlocked yet at boot time).

Version bumped to 0.8.7 (sync with coord + node).

## [2026-05-10] fix(deps): @tauri-apps/api ^2.11 to match Cargo tauri 2.11 (d20ca3e)

The 0.8.6 release CI failed with the Tauri version-mismatch
guard: `tauri (v2.11.1) : @tauri-apps/api (v2.10.1)`. Bumped
the npm side to `^2.11` so pnpm resolves matching major.minor.
Folded into the 0.8.6 tag (force-retagged `node-ui-v0.8.6` at
this commit; no release was published yet so no upstream
artifact churn).

## [2026-05-10] chore(security): tauri 2.11 + locator hardening + version 0.8.5 → 0.8.6 (d670a8f)

Closes Tauri GHSA-7gmj-67g7-phm9 (CVSS 6.1) by bumping
`tauri = "2.11"` in `src-tauri/Cargo.toml` (was `"2"` resolving to
2.10.3). Cargo.lock refreshed with the matching tauri-build,
tauri-codegen, tauri-macros, tauri-runtime, tauri-runtime-wry,
tauri-utils, tao, wry, muda, tray-icon updates.

Two source-level locator hardenings (`src-tauri/src/commands.rs`)
addressing reviewer findings on the just-shipped auto-install path:

- **M-1**: the `SYNAPSEIA_NODE_PATH` env override (dev-mode only)
  is now gated behind `#[cfg(debug_assertions)]`. Release builds
  no longer honor the env var, removing an attack surface where a
  user-writable shell rc could redirect the desktop app to a
  hostile `dist/index.js`.
- **M-2**: the `npm root -g` and hardcoded `npm_roots` branches
  now content-validate the resolved `package.json` (substring
  match on `"name": "@synapseia-network/node"`) before trusting
  the install. Defends against a malicious npm shim earlier on
  PATH or a write-where attacker on the global node_modules dir.

Version bumped to 0.8.6 to keep coord/node/node-ui in sync.

## [2026-05-10] feat(autoinstall): npm-install @synapseia-network/node CLI when missing (2155d6f)

The desktop app now auto-installs the `@synapseia-network/node` CLI
from the public npm registry when it can't locate it on disk. New
Tauri command `install_synapseia_node` runs `npm install -g
@synapseia-network/node` via `tokio::task::spawn_blocking`, emits
`install-progress` events to the UI, detects EACCES with a sudo
hint, and re-verifies the install before reporting success. The
locator (`find_synapseia_node`) gained an `npm root -g` fallback so
non-standard global prefixes (nvm/volta/fnm) are picked up. The
frontend wires a new `installing-node` boot phase: if `start_node`
fails with the typed error code `ERR_CLI_MISSING`, the UI auto-
triggers the installer, shows a spinner + progress text, and
retries `start_node` on completion. Re-entrancy guarded on both
sides — `tokio::sync::Mutex` on the Rust side, `installingRef` on
the React side. Locator branches now require BOTH `dist/index.js`
AND `package.json` to defeat partial-write races during install.
Also updates the `@synapseia/node` literal references throughout
the locator + install paths to the new `@synapseia-network/node`
package name.

## [2026-05-09] chore(version): align to 0.8.5 with coord + node (c146628)

Version-only bump 0.8.4 → 0.8.5 across `package.json`,
`src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` (and
`Cargo.lock` regenerated). Brings the package back into lockstep
with `coordinator` and `node` per the version-sync invariant. No
code change.

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
