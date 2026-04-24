# Changelog — node-ui

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
