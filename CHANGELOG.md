# Changelog — node-ui

## [2026-04-24] fix(shutdown): reliable node kill on app close

- Replace `try_lock()` with `blocking_lock()` in `reap_on_exit` so the node
  child is always reaped even when a background task (log streaming, status
  poll) holds the mutex at the moment the window closes.
- Add SIGTERM → 2 s grace → SIGKILL sequence on Unix so the Node.js process
  can flush logs and close WebSocket connections before being force-killed.
- `cleanup_lock_if_ours` now runs on the clean-exit path too, preventing a
  stale `~/.synapseia/node.lock` after graceful shutdown.
