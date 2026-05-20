use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;

// Hard cap for quick, read-only CLI invocations (wallet-verify, config, etc).
// 30 s is generous for a NestJS cold start but prevents an indefinite UI hang.
const CLI_TIMEOUT: Duration = Duration::from_secs(30);

// On-chain write commands — stake/unstake/claim/deposit/withdraw — block on
// `confirmed` commitment which routinely takes 20-60 s. Anything less than
// ~90 s produces spurious "command timed out" errors for txs that DID land.
const ON_CHAIN_CLI_TIMEOUT: Duration = Duration::from_secs(120);

/// Stable error code for "the @synapseia-network/node CLI is not present on
/// disk". The frontend matches on this string (not the human-readable suffix)
/// to decide whether to trigger the auto-install fallback.
pub const ERR_CLI_MISSING: &str = "ERR_CLI_MISSING";

/// Pinned Node.js LTS version downloaded when the user has no system node.
/// Bump deliberately when LTS rolls forward — the bundled runtime never
/// auto-updates after first install. Verified live on
/// https://nodejs.org/dist/ at the time of this change.
const BUNDLED_NODE_VERSION: &str = "22.20.0";

/// The UI build's own version, used as a hard floor in `check_cli_freshness`.
/// When the installed CLI is strictly older than this, force an upgrade even
/// if the npm registry is unreachable or returns a lower `latest` (e.g. a
/// rollback via `npm dist-tag`). Keeps node + node-ui locked at the same
/// semver — see the lockstep release rule in CLAUDE.md.
const MIN_NODE_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// npm registry dist-tags endpoint for `@synapseia-network/node`. The path
/// uses URL-encoded `%2F` because the registry's `/-/package/<name>/dist-tags`
/// route does not accept the unencoded `/` between scope and name.
const NPM_DIST_TAGS_URL: &str =
    "https://registry.npmjs.org/-/package/@synapseia-network%2Fnode/dist-tags";

/// Hard timeout for the npm registry dist-tags fetch. Short on purpose: this
/// gates a foreground install path. If npm is slow or unreachable the floor
/// logic (`MIN_NODE_CLI_VERSION`) still produces a correct decision.
const NPM_DIST_TAGS_TIMEOUT: Duration = Duration::from_secs(5);

/// Serializes concurrent `install_synapseia_node` invocations so a double-click
/// on Start can't fire two parallel `npm install -g` runs. The second waiter
/// re-checks `find_synapseia_node()` after the lock releases and returns the
/// already-installed path.
static INSTALL_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
fn install_lock() -> &'static tokio::sync::Mutex<()> {
    INSTALL_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

// Separate from INSTALL_LOCK on purpose: the install path holds INSTALL_LOCK
// across an `npm install -g` and then calls `ensure_node_runtime`, which is
// not reentrant on tokio::sync::Mutex — sharing one lock would deadlock.
// This lock guards the bundled-node download/extract so a future caller that
// reaches `ensure_node_runtime` outside the install flow can't race a
// concurrent install on the same staging dir.
static NODE_RUNTIME_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
fn node_runtime_lock() -> &'static tokio::sync::Mutex<()> {
    NODE_RUNTIME_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Serializes concurrent `install_python_deps` invocations so a double-click
/// on the loading screen can't fire two parallel pip-install runs. Separate
/// from INSTALL_LOCK / NODE_RUNTIME_LOCK because those guard the CLI npm
/// install / bundled node download — Python deps are an orthogonal step.
static PYTHON_INSTALL_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
fn python_install_lock() -> &'static tokio::sync::Mutex<()> {
    PYTHON_INSTALL_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Commands that send on-chain transactions and must use the longer timeout.
const ON_CHAIN_COMMANDS: &[&str] = &[
    "stake",
    "unstake",
    "claim-rewards",
    "claim-wo-rewards",
    "withdraw-sol",
    "withdraw-syn",
    "deposit-sol",
    "deposit-syn",
];

fn timeout_for(command: &str) -> Duration {
    if ON_CHAIN_COMMANDS.contains(&command) {
        ON_CHAIN_CLI_TIMEOUT
    } else {
        CLI_TIMEOUT
    }
}

// ─── run_command allowlist + per-command argv validation ─────────────────────
//
// SECURITY (F-node-ui-001): `run_command` is invoked from React via Tauri IPC
// with a free-form `command` string and `args: Vec<String>`. The wallet
// password is injected into the spawned Node process via the
// `SYNAPSEIA_WALLET_PASSWORD` env var.
//
// Without an allowlist a renderer-side XSS / compromised dependency / devtools
// call could invoke arbitrary `node` flags through the positional `command`
// slot — e.g. `invoke('run_command', { command: '--inspect-brk=0.0.0.0:9229' })`
// — which would attach a remote debugger to a Node.js process holding the
// cached wallet password in its environment. With `withGlobalTauri: true`
// (F-006) any iframe could reach the same surface.
//
// Defence-in-depth:
//   1. Hard allowlist `ALLOWED_COMMANDS` of CLI subcommands actually invoked
//      by the React UI. Anything else is rejected before reaching `node`.
//   2. Per-command argv shape validation (`validate_command_args`) that
//      caps argc, restricts the alphabet of positional args, and only
//      permits the specific `--flag value` pairs each subcommand expects.
//   3. Any arg that begins with `-` is rejected unless explicitly listed as
//      an allowed flag for that subcommand — this blocks Node.js runtime
//      flag injection through the positional slot (`--inspect`, `--require`,
//      `--experimental-loader`, etc.).

/// CLI subcommands the React UI is permitted to invoke via `run_command`.
/// Derived from the call sites:
///   * MyNodePanel.tsx      → claim-wo-rewards
///   * WalletPanel.tsx      → withdraw-sol, withdraw-syn, export-key
///   * StakePanel.tsx       → stake, unstake
///   * SettingsPanel.tsx    → config (--show, --set-name, --set-model,
///                                    --set-llm-key, --set-llm-url, --set-rpc-url)
/// `node` runtime flags (`--inspect`, `-r`, etc.) MUST NOT appear here — they
/// would bypass the `-` prefix guard and re-open the remote-debugger attack.
const ALLOWED_COMMANDS: &[&str] = &[
    "stake",
    "unstake",
    "claim-rewards",
    "claim-wo-rewards",
    "withdraw-sol",
    "withdraw-syn",
    "deposit-sol",
    "deposit-syn",
    "export-key",
    "config",
];

/// Whitelisted `--flag` names for the `config` subcommand. Anything that
/// looks like a flag (starts with `-`) and is not in this list is rejected
/// up front, defeating attempts to smuggle `--inspect-brk` etc. through the
/// `config` positional slot.
const ALLOWED_CONFIG_FLAGS: &[&str] = &[
    "--show",
    "--set-name",
    "--set-model",
    "--set-llm-key",
    "--set-llm-url",
    "--set-rpc-url",
];

/// Maximum length of any single positional argument or `--flag` value.
/// Generous enough to cover Solana base58 addresses (≤ 44 chars), provider/
/// model slugs, JWT-shaped LLM API keys, and RPC URLs, while still keeping
/// the surface bounded.
const MAX_ARG_LEN: usize = 512;

/// True for ASCII positional-argument character classes: base58 (digits +
/// letters minus 0/O/I/l), numeric amounts (digits, `.`), and common URL /
/// model-slug punctuation. Deliberately conservative — anything outside the
/// set is rejected.
fn is_safe_positional_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '+' | '=' | '?' | '&' | '%')
}

/// Validate one positional argument: length-capped, ASCII-only, drawn from
/// `is_safe_positional_char`, and MUST NOT start with `-` (would be a flag).
fn validate_positional(arg: &str, ctx: &str) -> Result<(), String> {
    if arg.is_empty() {
        return Err(format!("Invalid {ctx}: empty argument"));
    }
    if arg.len() > MAX_ARG_LEN {
        return Err(format!("Invalid {ctx}: argument exceeds {MAX_ARG_LEN} bytes"));
    }
    if arg.starts_with('-') {
        return Err(format!(
            "Invalid {ctx}: positional arguments must not start with '-' (would be parsed as a flag)"
        ));
    }
    if !arg.chars().all(is_safe_positional_char) {
        return Err(format!(
            "Invalid {ctx}: argument contains characters outside the safe set"
        ));
    }
    Ok(())
}

/// Validate a `--flag value` pair for the `config` subcommand. `--show` is
/// a no-value flag and is handled separately by the caller.
fn validate_config_flag_value(flag: &str, value: &str) -> Result<(), String> {
    if !ALLOWED_CONFIG_FLAGS.contains(&flag) {
        return Err(format!("Invalid config flag: {flag}"));
    }
    if value.len() > MAX_ARG_LEN {
        return Err(format!(
            "Invalid config value for {flag}: exceeds {MAX_ARG_LEN} bytes"
        ));
    }
    // The CLI itself accepts `""` as a meaningful "clear this value" signal
    // for some flags (e.g. --set-rpc-url). Empty is allowed; non-empty must
    // be control-char-free.
    if value.chars().any(|c| c.is_control()) {
        return Err(format!(
            "Invalid config value for {flag}: contains control characters"
        ));
    }
    Ok(())
}

/// Apply the per-command argv shape rules. Returns `Ok(())` if `command +
/// args` is a legal shape the CLI is willing to receive from the UI.
///
/// Any argv shape the React UI does not actually produce is rejected here,
/// even if the underlying CLI would accept it. Tighten / loosen alongside
/// the call-site grep, never to make a new test pass.
fn validate_command_args(command: &str, args: &[String]) -> Result<(), String> {
    if !ALLOWED_COMMANDS.contains(&command) {
        return Err(format!(
            "Command `{command}` is not in the allowlist. Refusing to invoke."
        ));
    }

    match command {
        // Single positional <amount> (numeric SYN value, validated upstream
        // by the React UI; we only enforce shape here).
        "stake" | "unstake" => {
            if args.len() != 1 {
                return Err(format!(
                    "`{command}` expects exactly 1 argument (amount); got {}",
                    args.len()
                ));
            }
            validate_positional(&args[0], command)?;
        }

        // `withdraw-sol` / `withdraw-syn` accept either:
        //   [destination]                 (UI sends when amount field empty)
        //   [amount, destination]
        // Both positional. The CLI itself parses amount as a number and
        // destination as a Solana address — we just enforce shape.
        "withdraw-sol" | "withdraw-syn" => {
            if args.is_empty() || args.len() > 2 {
                return Err(format!(
                    "`{command}` expects 1 or 2 arguments; got {}",
                    args.len()
                ));
            }
            for a in args {
                validate_positional(a, command)?;
            }
        }

        // `deposit-sol <amount>` / `deposit-syn [amount]`. Currently not
        // wired from the UI but kept in the allowlist for parity with
        // ON_CHAIN_COMMANDS / future wiring.
        "deposit-sol" => {
            if args.len() != 1 {
                return Err(format!(
                    "`{command}` expects exactly 1 argument (amount); got {}",
                    args.len()
                ));
            }
            validate_positional(&args[0], command)?;
        }
        "deposit-syn" => {
            if args.len() > 1 {
                return Err(format!(
                    "`{command}` expects 0 or 1 argument; got {}",
                    args.len()
                ));
            }
            if let Some(a) = args.first() {
                validate_positional(a, command)?;
            }
        }

        // Zero-arg commands.
        "claim-rewards" | "claim-wo-rewards" | "export-key" => {
            if !args.is_empty() {
                return Err(format!(
                    "`{command}` expects no arguments; got {}",
                    args.len()
                ));
            }
        }

        // `config` is the only command that accepts flags. Allowed shapes:
        //   ["--show"]                   (one no-value flag)
        //   ["--set-foo", "<value>"]     (single setter call)
        // The UI never batches multiple setters into one invocation — it
        // loops and calls `config` once per flag (see SettingsPanel.tsx).
        "config" => {
            match args.len() {
                1 => {
                    if args[0] != "--show" {
                        return Err(format!(
                            "`config` with a single arg only accepts `--show`; got `{}`",
                            args[0]
                        ));
                    }
                }
                2 => {
                    validate_config_flag_value(&args[0], &args[1])?;
                    if args[0] == "--show" {
                        return Err(
                            "`config --show` does not take a value".to_string()
                        );
                    }
                }
                n => {
                    return Err(format!(
                        "`config` expects 1 or 2 arguments; got {n}"
                    ));
                }
            }
        }

        // Should be unreachable because of the ALLOWED_COMMANDS gate above,
        // but fail closed if the lists ever drift.
        _ => {
            return Err(format!(
                "Command `{command}` is allowlisted but has no argv validator. Refusing to invoke."
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod run_command_validation_tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rejects_unknown_command() {
        assert!(validate_command_args("rm", &s(&["-rf", "/"])).is_err());
        assert!(validate_command_args("--inspect-brk=0.0.0.0:9229", &[]).is_err());
        assert!(validate_command_args("-r", &s(&["evil.js"])).is_err());
    }

    #[test]
    fn rejects_node_flag_injection_through_positional() {
        // Caller tries to smuggle `--inspect` through a known subcommand.
        assert!(validate_command_args("stake", &s(&["--inspect-brk=0.0.0.0:9229"])).is_err());
        assert!(validate_command_args("withdraw-sol", &s(&["--require", "evil.js"])).is_err());
        assert!(validate_command_args("claim-wo-rewards", &s(&["-e", "process.exit()"])).is_err());
    }

    #[test]
    fn accepts_known_happy_paths() {
        assert!(validate_command_args("stake", &s(&["100"])).is_ok());
        assert!(validate_command_args("unstake", &s(&["50.5"])).is_ok());
        assert!(validate_command_args("claim-wo-rewards", &[]).is_ok());
        assert!(validate_command_args("export-key", &[]).is_ok());
        assert!(validate_command_args(
            "withdraw-sol",
            &s(&["0.1", "5xJ7sN8YbqXz3Wp2K9aL1Q4mF6tV8R2c"])
        )
        .is_ok());
        assert!(validate_command_args(
            "withdraw-syn",
            &s(&["5xJ7sN8YbqXz3Wp2K9aL1Q4mF6tV8R2c"])
        )
        .is_ok());
        assert!(validate_command_args("config", &s(&["--show"])).is_ok());
        assert!(
            validate_command_args("config", &s(&["--set-name", "my-node"])).is_ok()
        );
        assert!(validate_command_args(
            "config",
            &s(&["--set-model", "openai/gpt-4o-mini"])
        )
        .is_ok());
        assert!(validate_command_args(
            "config",
            &s(&["--set-rpc-url", "https://api.devnet.solana.com"])
        )
        .is_ok());
        // Empty value is permitted for `--set-rpc-url` (clear-to-default).
        assert!(validate_command_args("config", &s(&["--set-rpc-url", ""])).is_ok());
    }

    #[test]
    fn rejects_unknown_config_flag() {
        assert!(
            validate_command_args("config", &s(&["--set-keystore-path", "/etc/passwd"]))
                .is_err()
        );
        assert!(validate_command_args("config", &s(&["--show", "extra"])).is_err());
    }

    #[test]
    fn rejects_wrong_argc() {
        assert!(validate_command_args("stake", &[]).is_err());
        assert!(validate_command_args("stake", &s(&["1", "2"])).is_err());
        assert!(validate_command_args("withdraw-sol", &[]).is_err());
        assert!(
            validate_command_args("withdraw-sol", &s(&["a", "b", "c"])).is_err()
        );
        assert!(validate_command_args("export-key", &s(&["leak"])).is_err());
    }

    #[test]
    fn rejects_control_chars_and_oversized_values() {
        let big = "a".repeat(MAX_ARG_LEN + 1);
        assert!(validate_command_args("stake", &[big]).is_err());
        assert!(
            validate_command_args("config", &s(&["--set-name", "evil\nname"])).is_err()
        );
    }
}

pub struct NodeProcess {
    child: Option<Child>,
    logs: Arc<Mutex<Vec<String>>>,
    /// Cached on the original `start_node` call so the auto-respawn path after
    /// a self-update can re-spawn the CLI without prompting the user for the
    /// wallet password again. Cleared on `stop_node` so a manual stop always
    /// re-requires the password on next start.
    ///
    /// F-node-ui-005 (P36): wrapped in `Zeroizing<String>` so the password
    /// bytes are scrubbed when the field is reassigned or dropped. Without
    /// this, a core dump or post-exit memory inspection (e.g. swap file,
    /// /proc/<pid>/maps prior to release) could leak the wallet password in
    /// plaintext. `mlock` is left as a future hardening — the trade-off is
    /// platform-specific RLIMIT_MEMLOCK pressure and is documented inline.
    cached_password: Option<zeroize::Zeroizing<String>>,
    /// Latched true when the CLI emits `[SELF_UPDATE_RESTART]` on stdout
    /// (see packages/node/src/utils/self-updater.ts). The stdout reader
    /// flips it; the EOF handler checks it to decide between auto-respawn
    /// and "user-visible crash". Cleared after the respawn is dispatched
    /// (success or failure) so a subsequent genuine crash isn't mistaken
    /// for a self-update.
    pending_self_update_restart: bool,
    /// Monotonically incremented every time we spawn a node child. The stdout
    /// EOF watcher captures the generation it was spawned for and only
    /// triggers the respawn path if state.generation still matches — protects
    /// against a manual stop+restart race where the OLD EOF handler would
    /// otherwise spawn a third child on top of the user's fresh start.
    generation: u64,
    /// Per-spawn random nonce injected into the child via
    /// `SYNAPSEIA_SELF_UPDATE_NONCE`. Only the legitimate child knows this
    /// value, so only the legitimate child can emit a `[SELF_UPDATE_RESTART]`
    /// marker that survives `parse_self_update_cue_with_nonce` (F-node-ui-004).
    /// Rotated on every spawn. Empty when no child is alive.
    self_update_nonce: String,
}

impl NodeProcess {
    pub fn new() -> Self {
        Self {
            child: None,
            logs: Arc::new(Mutex::new(Vec::new())),
            cached_password: None,
            pending_self_update_restart: false,
            generation: 0,
            self_update_nonce: String::new(),
        }
    }
}

pub type NodeProcessState = Arc<Mutex<NodeProcess>>;

/// Detect the self-update relaunch cue emitted by the CLI just before exit
/// (`console.log('[SELF_UPDATE_RESTART] nonce=... v... pid=...')` in
/// packages/node/src/utils/self-updater.ts). Extracted so it can be unit
/// tested without spawning a real process.
///
/// F-node-ui-004 (P10) — substring `[SELF_UPDATE_RESTART]` is no longer
/// enough. Any process output (work-order body, KG ingest, web search
/// result, log forwarder echo) could carry the literal marker substring
/// and force a spurious respawn. We require a strict shape that ONLY a
/// legitimate child we spawned can emit, because we inject the nonce
/// via `SYNAPSEIA_SELF_UPDATE_NONCE` env at spawn time.
///
/// Shape (single line, anchored both ends after trimming any timestamp
/// prefix at the START of the line):
///
///   `[SELF_UPDATE_RESTART] nonce=<hex>  v<semver>  pid=<digits>` — body.
///
/// The `nonce` value must match the per-spawn `expected_nonce` exactly.
/// `expected_nonce` is the hex string this UI process generated and
/// passed to the child via env at spawn time.
pub fn parse_self_update_cue(line: &str) -> bool {
    // Kept for back-compat with existing call sites that pass the empty
    // nonce (e.g. legacy tests). Always returns false — the strict path
    // requires a real nonce.
    parse_self_update_cue_with_nonce(line, "")
}

/// Strict variant of `parse_self_update_cue`. Accepts the canonical
/// marker shape only when the embedded `nonce=<value>` matches
/// `expected_nonce` (non-empty). Empty `expected_nonce` always returns
/// false — protects against the "no env, accept anything" footgun.
pub fn parse_self_update_cue_with_nonce(line: &str, expected_nonce: &str) -> bool {
    if expected_nonce.is_empty() {
        return false;
    }

    // Strip a leading timestamp / log-forwarder prefix if present. The
    // marker itself starts with `[SELF_UPDATE_RESTART]`; everything
    // before it is the forwarder's preamble and may include arbitrary
    // bytes. We find the bracketed token once and parse what follows.
    let Some(idx) = line.find("[SELF_UPDATE_RESTART]") else {
        return false;
    };
    let after = line[idx + "[SELF_UPDATE_RESTART]".len()..].trim();

    // Tokenize by ASCII whitespace. Order is fixed:
    //   nonce=<hex>  v<semver>  pid=<digits>
    // Anything else, in any other order, with extra tokens or missing
    // tokens, fails closed.
    let parts: Vec<&str> = after.split_ascii_whitespace().collect();
    if parts.len() != 3 {
        return false;
    }
    let Some(nonce_value) = parts[0].strip_prefix("nonce=") else {
        return false;
    };
    if nonce_value != expected_nonce {
        return false;
    }
    // Validate v<semver> shape: 'v' followed by digits.dot.digits.dot.digits.
    let Some(ver) = parts[1].strip_prefix('v') else {
        return false;
    };
    let ver_parts: Vec<&str> = ver.split('.').collect();
    if ver_parts.len() != 3 || !ver_parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    // Validate pid=<digits>.
    let Some(pid_value) = parts[2].strip_prefix("pid=") else {
        return false;
    };
    if pid_value.is_empty() || !pid_value.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    true
}

/// Generate a 32-hex-character (16-byte) random nonce for the
/// `[SELF_UPDATE_RESTART]` marker. Uses the OS CSPRNG via the
/// `getrandom` crate (already transitive). Falls back to a
/// pid+time-based bytes if the syscall fails; the fallback is still
/// hard for an unrelated stdout-line generator to guess because it
/// embeds a high-resolution timestamp the attacker doesn't observe.
fn generate_self_update_nonce() -> String {
    let mut buf = [0u8; 16];
    if getrandom::getrandom(&mut buf).is_err() {
        // Best-effort fallback: mix the current process id, monotonic
        // nanos and a syscall-failure flag. Not cryptographic strength,
        // but the attacker observing only the stdout marker substring
        // still can't know the nanos at spawn time.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let mixed = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(pid);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((mixed >> (i * 8)) & 0xff) as u8;
        }
    }
    let mut s = String::with_capacity(32);
    for b in &buf {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeStatus {
    pub running: bool,
    pub peer_id: Option<String>,
    pub tier: Option<u32>,
    pub wallet: Option<String>,
    pub balance_sol: Option<f64>,
    pub balance_syn: Option<f64>,
    pub staked_syn: Option<f64>,
    pub pid: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommandResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LogLine {
    pub timestamp: String,
    pub level: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UnlockResult {
    pub success: bool,
    pub wallet_address: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WalletExists {
    pub exists: bool,
    pub wallet_path: String,
    pub config_exists: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExternalNodeInfo {
    /// True when a node is running outside of this Tauri process's control.
    /// i.e. the lock file exists, the PID is alive, and it isn't the child
    /// we spawned ourselves.
    pub external: bool,
    pub pid: Option<u32>,
    pub source: Option<String>,
    pub started_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub os: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub ram_gb: f64,
    pub gpu_type: Option<String>,
    pub gpu_vram_gb: Option<f64>,
    pub recommended_tier: u8,
    pub has_ollama: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChainInfo {
    pub wallet: Option<String>,
    pub sol: f64,
    pub syn: f64,
    // Direct on-chain StakeAccount reads (matches the dashboard).
    pub staked: f64,
    pub rewards_pending: f64,
    pub stake_account_exists: bool,
    pub stake_locked_until: i64,
    pub token_account_exists: bool,
    pub coordinator_reachable: bool,
    // On-chain RewardAccount PDA (syn_rewards_vault program) + coordinator
    // breakdown per reward type. This is the pool the dashboard's Claim
    // button targets.
    pub vault_claimable_syn: f64,
    #[serde(default)]
    pub rewards_by_type: std::collections::HashMap<String, f64>,
    // Stats pulled from the coordinator — zero when the node is unknown.
    pub presence_points: f64,
    pub total_wins: f64,
    pub total_submissions: f64,
    pub unclaimed_syn: f64,
    pub total_claimed_syn: f64,
    pub canary_strikes: f64,
    pub anomaly_warnings: f64,
    pub attestation_failures: f64,
    pub tier: Option<i32>,
    pub node_name: Option<String>,
}

// ─── Wallet lifecycle ────────────────────────────────────────────────────────
// The session password lives in React memory only and is never persisted by
// the Rust side. If a future version wants keychain-backed auto-unlock, add
// the `keyring` crate commands back — they were removed to avoid a deceptive
// "password stored somewhere" surface.

fn synapseia_home() -> PathBuf {
    if let Ok(dir) = std::env::var("SYNAPSEIA_HOME") {
        return PathBuf::from(dir);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".synapseia")
}

#[tauri::command]
pub async fn wallet_exists() -> Result<WalletExists, String> {
    let home = synapseia_home();
    let wallet_path = home.join("wallet.json");
    let config_path = home.join("config.json");
    Ok(WalletExists {
        exists: wallet_path.exists(),
        wallet_path: wallet_path.to_string_lossy().to_string(),
        config_exists: config_path.exists(),
    })
}

// ─── UI-only settings (Ollama endpoint override, etc.) ────────────────────────
//
// These are NOT persisted in the node CLI's config.json on purpose: the node
// CLI honors the `OLLAMA_URL` env var across embeddings, hardware, ollama,
// and training-llm modules, so the desktop UI persists the override locally
// and injects it as an env var whenever it spawns the CLI. Keeps the CLI
// surface unchanged and avoids fighting the deprecated `--llm-url` flag.

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq)]
pub struct UiSettings {
    /// Empty string means "use the built-in default
    /// (http://localhost:11434)" — never persisted as a literal fallback.
    #[serde(default, rename = "ollamaUrl")]
    pub ollama_url: String,
    /// LLM provider id selected in the desktop UI. Empty string or
    /// "ollama" (case-insensitive) means "use local Ollama". Any other
    /// value (e.g. "nvidia", "openai") is treated as a cloud provider
    /// and forwarded to the spawned CLI via LLM_CLOUD_PROVIDER + the
    /// corresponding <PROVIDER>_API_KEY env var.
    #[serde(default, rename = "llmProvider")]
    pub llm_provider: String,
    /// Cloud model slug (the part AFTER the provider/ prefix). Empty
    /// string when the operator picked the Ollama provider, since the
    /// model is part of the slug stored in the CLI config in that case.
    #[serde(default, rename = "llmModelSlug")]
    pub llm_model_slug: String,
    /// Plaintext API key for the chosen cloud provider. Persisted in
    /// ui-settings.json with 0o600 perms on Unix. Empty string means
    /// "not configured" (operator can still set the env var externally).
    #[serde(default, rename = "llmApiKey")]
    pub llm_api_key: String,
}

fn ui_settings_path() -> PathBuf {
    synapseia_home().join("ui-settings.json")
}

fn read_ui_settings_raw() -> UiSettings {
    let path = ui_settings_path();
    if !path.exists() {
        return UiSettings::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<UiSettings>(&raw).unwrap_or_default(),
        Err(_) => UiSettings::default(),
    }
}

fn write_ui_settings_raw(settings: &UiSettings) -> Result<(), String> {
    let home = synapseia_home();
    if !home.exists() {
        std::fs::create_dir_all(&home).map_err(|e| {
            format!(
                "failed to create {}: {}",
                home.to_string_lossy(),
                e
            )
        })?;
    }
    let path = ui_settings_path();
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("failed to serialize ui-settings: {}", e))?;
    std::fs::write(&path, json).map_err(|e| {
        format!(
            "failed to write {}: {}",
            path.to_string_lossy(),
            e
        )
    })?;
    // The file holds a plaintext cloud LLM API key. On Unix, pin it to
    // 0o600 (owner read/write only) so a multi-user box doesn't expose
    // the credential to other accounts. Windows inherits the
    // %USERPROFILE% ACL which is already user-restricted by default;
    // pinning ACLs here would require an extra crate and is out of
    // scope for this slice.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        // Best-effort: a perm-set failure should not abort the write
        // (the file is already on disk with default perms; logging
        // surfaces the problem without breaking the user flow).
        if let Err(e) = std::fs::set_permissions(&path, perms) {
            log::warn!(
                "failed to chmod 600 on {}: {}",
                path.to_string_lossy(),
                e
            );
        }
    }
    Ok(())
}

/// Map a provider id (as stored in ui-settings.json) to the env var the
/// node CLI reads to authenticate against that provider. Mirrors the
/// `apiKeyEnvVar` table in `packages/node-ui/src/lib/providers.ts` and
/// `packages/node/src/modules/llm/providers.ts`. Returns None for
/// unknown ids (caller skips API-key injection in that case).
pub(crate) fn api_key_env_for_provider(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => Some("OPENAI_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "google" => Some("GEMINI_API_KEY"),
        "moonshot" => Some("MOONSHOT_API_KEY"),
        "minimax" => Some("MINIMAX_API_KEY"),
        "zhipu" => Some("ZHIPU_API_KEY"),
        "nvidia" => Some("NVIDIA_API_KEY"),
        _ => None,
    }
}

/// Resolve the cloud LLM env vars to inject when spawning the node CLI.
/// Returns None when the operator picked Ollama (the local path), or
/// when the persisted selection is incomplete (no provider, or no model
/// slug). Each tuple element is (env_name, env_value). Empty API keys
/// are still emitted so the CLI surfaces a clear "missing key" error
/// instead of silently falling back to a different provider.
pub(crate) fn cloud_llm_env_for(settings: &UiSettings) -> Option<Vec<(String, String)>> {
    let provider = settings.llm_provider.trim();
    let slug = settings.llm_model_slug.trim();
    if provider.is_empty() || slug.is_empty() {
        return None;
    }
    if provider.eq_ignore_ascii_case("ollama") {
        return None;
    }
    let mut env: Vec<(String, String)> = Vec::with_capacity(4);
    env.push(("LLM_PROVIDER".to_string(), "cloud".to_string()));
    env.push(("LLM_CLOUD_PROVIDER".to_string(), provider.to_string()));
    env.push(("LLM_CLOUD_MODEL".to_string(), slug.to_string()));
    if let Some(key_env) = api_key_env_for_provider(provider) {
        env.push((key_env.to_string(), settings.llm_api_key.clone()));
    }
    Some(env)
}

#[tauri::command]
pub async fn get_ui_settings() -> Result<UiSettings, String> {
    Ok(read_ui_settings_raw())
}

#[tauri::command]
pub async fn set_ui_settings(
    ollama_url: String,
    llm_provider: Option<String>,
    llm_model_slug: Option<String>,
    llm_api_key: Option<String>,
) -> Result<UiSettings, String> {
    let trimmed = ollama_url.trim();
    // Soft URL shape validation here mirrors the JS-side check so a malformed
    // value can't sneak in via a direct invoke() call from devtools.
    if !trimmed.is_empty() {
        if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
            return Err("Ollama URL must start with http:// or https://".to_string());
        }
        if trimmed.ends_with('/') {
            return Err("Ollama URL must not end with a trailing slash".to_string());
        }
    }
    // Merge: each None field leaves the previously-persisted value
    // untouched so the frontend can update partial subsets (e.g. only
    // the API key) without round-tripping the rest. Some("") explicitly
    // clears a field — that is the path the UI takes when the operator
    // switches back from a cloud provider to Ollama.
    let current = read_ui_settings_raw();
    let next = UiSettings {
        ollama_url: trimmed.to_string(),
        llm_provider: llm_provider.unwrap_or(current.llm_provider),
        llm_model_slug: llm_model_slug.unwrap_or(current.llm_model_slug),
        llm_api_key: llm_api_key.unwrap_or(current.llm_api_key),
    };
    write_ui_settings_raw(&next)?;
    Ok(next)
}

/// Hardware introspection without spawning anything. Uses the `sysinfo`
/// crate directly so the SystemPanel is instant — no CLI bootstrap, no
/// coordinator round trip. GPU detection is macOS-specific via `system_profiler`.
#[tauri::command]
pub async fn system_info() -> Result<SystemInfo, String> {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_cpu_all();
    sys.refresh_memory();

    let cpu_model = sys
        .cpus()
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    let cpu_cores = sys.cpus().len() as u32;
    let ram_gb = (sys.total_memory() as f64) / 1_073_741_824.0;
    let os = format!(
        "{} {}",
        System::name().unwrap_or_else(|| "Unknown".to_string()),
        System::os_version().unwrap_or_default()
    );

    let (gpu_type, gpu_vram_gb) = detect_gpu();
    let has_ollama = which_in_path("ollama").is_some();

    // Tier recommendation mirrors packages/node/src/modules/hardware — a
    // rough VRAM bucketing so users see "Tier 2 · 16GB GPU" not a raw number.
    let vram = gpu_vram_gb.unwrap_or(0.0);
    let recommended_tier: u8 = if vram >= 80.0 {
        5
    } else if vram >= 32.0 {
        4
    } else if vram >= 24.0 {
        3
    } else if vram >= 16.0 {
        2
    } else if vram >= 8.0 {
        1
    } else {
        0
    };

    Ok(SystemInfo {
        os,
        cpu_model,
        cpu_cores,
        ram_gb,
        gpu_type,
        gpu_vram_gb,
        recommended_tier,
        has_ollama,
    })
}

#[cfg(target_os = "macos")]
fn detect_gpu() -> (Option<String>, Option<f64>) {
    // system_profiler SPDisplaysDataType prints one of many shapes; we grab
    // the first "Chipset Model:" and "VRAM (Total):" / "Metal Support:" lines.
    let Ok(out) = std::process::Command::new("system_profiler")
        .arg("SPDisplaysDataType")
        .output()
    else {
        return (None, None);
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut chipset: Option<String> = None;
    let mut vram: Option<f64> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if chipset.is_none() {
            if let Some(rest) = line.strip_prefix("Chipset Model:") {
                chipset = Some(rest.trim().to_string());
            }
        }
        if vram.is_none() {
            if let Some(rest) = line
                .strip_prefix("VRAM (Total):")
                .or_else(|| line.strip_prefix("VRAM (Dynamic, Max):"))
            {
                vram = parse_vram_gb(rest.trim());
            }
        }
    }
    // Apple Silicon has no discrete VRAM; Metal uses unified memory. Fall
    // back to total RAM so tier recommendation still works there.
    if chipset.is_some() && vram.is_none() {
        // Conservative: report unified memory size as VRAM for tier purposes.
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        vram = Some((sys.total_memory() as f64) / 1_073_741_824.0);
    }
    (chipset, vram)
}

#[cfg(not(target_os = "macos"))]
fn detect_gpu() -> (Option<String>, Option<f64>) {
    (None, None)
}

fn parse_vram_gb(s: &str) -> Option<f64> {
    // e.g. "8 GB", "8192 MB"
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.len() < 2 {
        return None;
    }
    let n: f64 = tokens[0].parse().ok()?;
    match tokens[1].to_uppercase().as_str() {
        "GB" => Some(n),
        "MB" => Some(n / 1024.0),
        "KB" => Some(n / 1_048_576.0),
        _ => None,
    }
}

/// Lightweight chain poll. Spawns the node CLI's `chain-info` subcommand,
/// which short-circuits BEFORE NestJS bootstrap and queries Solana directly.
/// No P2P handshakes, no heartbeats, no coordinator noise — just the numbers.
#[tauri::command]
pub async fn fetch_chain_info(app: AppHandle) -> Result<ChainInfo, String> {
    let mut cmd = build_node_command(Some(&app), &["chain-info"])?;
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(Stdio::null());

    let output = match tokio::time::timeout(CLI_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("spawn failed: {}", e)),
        Err(_) => return Err(format!("chain-info timed out after {}s", CLI_TIMEOUT.as_secs())),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    // The helper emits exactly one `__CHAIN_INFO__ {json}` line. Scan for it
    // so any pre-line noise (e.g. the `bigint: Failed to load bindings`
    // startup warning on Node 22+) is ignored.
    for line in stdout.lines() {
        if let Some(idx) = line.find("__CHAIN_INFO__") {
            let json = line[idx + "__CHAIN_INFO__".len()..].trim();
            return serde_json::from_str::<ChainInfo>(json)
                .map_err(|e| format!("failed to parse chain-info payload: {}", e));
        }
    }

    let stderr = strip_known_noise(&String::from_utf8_lossy(&output.stderr));
    Err(format!(
        "chain-info produced no __CHAIN_INFO__ line (stderr: {})",
        stderr.lines().last().unwrap_or("<empty>")
    ))
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CapacityResponse {
    pub limit: u64,
    pub current: u64,
    pub accepting: bool,
}

/// Resolve the coordinator URL the desktop UI should hit for read-only
/// pre-flight calls (capacity gate, etc.). The URL is no longer
/// user-configurable from the UI or from `~/.synapseia/config.json` —
/// any legacy `coordinatorUrl` value on disk is ignored. Reads the
/// `COORDINATOR_URL` env var with the official Synapseia coordinator as
/// the hardcoded fallback. This MUST stay in lockstep with
/// `packages/node/src/constants/coordinator.ts::OFFICIAL_COORDINATOR_URL`.
fn read_coordinator_url() -> String {
    std::env::var("COORDINATOR_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://api.synapseia.network".to_string())
}

/// Pre-flight the closed-beta capacity gate by hitting the coordinator's
/// public `GET /peer/capacity` endpoint. Returns the full payload so the
/// frontend can show "X / Y nodes registered" alongside the
/// limit-reached modal. Network failures bubble up as `Err`; the caller
/// is expected to fall through to the normal `start_node` path on error
/// rather than show a false-positive beta-limit modal.
#[tauri::command]
pub async fn check_capacity() -> Result<CapacityResponse, String> {
    let coordinator_url = read_coordinator_url();
    let url = format!("{}/peer/capacity", coordinator_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("coord returned status {}", response.status()));
    }

    response
        .json::<CapacityResponse>()
        .await
        .map_err(|e| format!("invalid capacity response: {}", e))
}

/// Report whether a Synapseia node is running *outside* this Tauri process.
/// We read ~/.synapseia/node.lock (written by the CLI `start` command or by
/// our own start_node) and, if the PID is still alive, return it — but ONLY
/// if it isn't our own child. The Dashboard polls this to show a banner when
/// the user starts a node from the terminal while the desktop app is open.
#[tauri::command]
pub async fn external_node_info(
    state: State<'_, NodeProcessState>,
) -> Result<ExternalNodeInfo, String> {
    let our_pid = {
        let node = state.lock().await;
        node.child.as_ref().and_then(|c| c.id())
    };
    Ok(read_external_lock(our_pid).unwrap_or(ExternalNodeInfo {
        external: false,
        pid: None,
        source: None,
        started_at: None,
    }))
}

#[derive(Debug, Deserialize)]
struct LockFile {
    pid: u32,
    #[serde(rename = "startedAt")]
    started_at: String,
    source: String,
}

fn read_external_lock(our_pid: Option<u32>) -> Option<ExternalNodeInfo> {
    let lock_path = synapseia_home().join("node.lock");
    if !lock_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&lock_path).ok()?;
    let lock: LockFile = serde_json::from_str(&content).ok()?;
    if !is_pid_alive(lock.pid) {
        // Stale — let the CLI clean it up on next start; we just ignore.
        return None;
    }
    if our_pid == Some(lock.pid) {
        // The running node is our own child, not external.
        return None;
    }
    Some(ExternalNodeInfo {
        external: true,
        pid: Some(lock.pid),
        source: Some(lock.source),
        started_at: Some(lock.started_at),
    })
}

// Signal constants — gated by cfg(unix) because libc on Windows does not
// expose SIGTERM/SIGKILL (Windows uses different process termination APIs).
// The cfg(not(unix)) placeholders preserve the call-site signature so the
// code compiles cross-platform; send_signal on Windows always returns Err.
#[cfg(unix)]
const SIG_TERM: libc::c_int = libc::SIGTERM;
#[cfg(not(unix))]
const SIG_TERM: i32 = 15;
#[cfg(unix)]
const SIG_KILL: libc::c_int = libc::SIGKILL;
#[cfg(not(unix))]
const SIG_KILL: i32 = 9;

fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // Direct libc::kill(pid, 0) syscall — signal 0 is a no-op probe that
    // returns 0 if the process exists and we have permission to signal it,
    // -1 + ESRCH if dead, -1 + EPERM if alive but un-signalable. Runs in
    // ~1µs vs ~10-30ms for a fork+exec to /bin/kill on macOS. Polled every
    // 3s so the difference is measurable in steady-state CPU.
    #[cfg(unix)]
    {
        // SAFETY: `kill` with signal 0 has no side effects on the target
        // process; it only performs an existence + permission check.
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if ret == 0 {
            return true;
        }
        // EPERM means the process exists but we can't signal it (different
        // user / privileged). Treat as alive for our purposes.
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return errno == libc::EPERM;
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Detailed view of `~/.synapseia/node.lock` used by the desktop UI to render
/// the LockBanner. Differs from `external_node_info` in two ways:
///   - reports BOTH alive and dead (zombie) lock states so the UI can offer a
///     "clean stale lock" path.
///   - returns `age_seconds` so the UI can render a "running for 12m" subtext
///     without doing date math in TypeScript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalLockInfo {
    pub pid: u32,
    /// "cli" | "ui" | "unknown" — sourced from the lock file. Unknown values
    /// are surfaced verbatim so a future CLI tag (e.g. "operator") doesn't
    /// silently collapse to "cli".
    pub source: String,
    /// ISO 8601 timestamp written by the lock owner.
    pub started_at: String,
    /// Seconds elapsed since `started_at`. Computed at read time. 0 if the
    /// timestamp is unparseable (rare; the CLI uses `Date.toISOString()`).
    pub age_seconds: u64,
    /// True iff `kill(pid, 0)` returns Ok — meaning the PID is alive and
    /// signalable. A dead PID = zombie lock that the UI can clean up.
    pub is_alive: bool,
}

/// Read `~/.synapseia/node.lock` and report both alive and dead states. The UI
/// polls this every 3 s to render the LockBanner. Returning `None` means
/// "no banner" (no lock file at all). Returning `Some` with `is_alive=false`
/// means "stale lock, offer clean action".
///
/// Cheap: one FS stat + one JSON parse + one `kill(pid, 0)` syscall.
#[tauri::command]
pub async fn check_external_lock() -> Option<ExternalLockInfo> {
    let lock_path = synapseia_home().join("node.lock");
    if !lock_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&lock_path).ok()?;
    let lock: LockFile = serde_json::from_str(&content).ok()?;
    let is_alive = is_pid_alive(lock.pid);
    let age_seconds = parse_iso8601_age_seconds(&lock.started_at);
    let source = if lock.source == "cli" || lock.source == "ui" {
        lock.source
    } else {
        "unknown".to_string()
    };
    Some(ExternalLockInfo {
        pid: lock.pid,
        source,
        started_at: lock.started_at,
        age_seconds,
        is_alive,
    })
}

/// Compute elapsed seconds between an ISO 8601 timestamp and now. Returns 0
/// on parse failure rather than `Option<u64>` because the UI just renders a
/// "running for 0s" subtext in that case — strictly better than hiding the
/// banner over a timestamp typo.
fn parse_iso8601_age_seconds(iso: &str) -> u64 {
    match chrono::DateTime::parse_from_rfc3339(iso) {
        Ok(dt) => {
            let now = chrono::Utc::now();
            let elapsed = now.signed_duration_since(dt.with_timezone(&chrono::Utc));
            if elapsed.num_seconds() < 0 {
                0
            } else {
                elapsed.num_seconds() as u64
            }
        }
        Err(_) => 0,
    }
}

/// Forcefully release `~/.synapseia/node.lock`. If the lock-holding PID is
/// alive, send SIGTERM, wait up to 5 s for graceful exit, then SIGKILL.
/// Finally remove the lock file.
///
/// Safety (P2 fail-closed): we re-read the lock RIGHT BEFORE sending the
/// signal. If the PID changed between the UI's `check_external_lock` poll and
/// this command (race: original process exited, a new one claimed the lock),
/// we abort with Err rather than killing the new owner.
#[tauri::command]
pub async fn force_release_lock() -> Result<(), String> {
    let lock_path = synapseia_home().join("node.lock");
    if !lock_path.exists() {
        // Idempotent: nothing to do.
        return Ok(());
    }
    let content = std::fs::read_to_string(&lock_path)
        .map_err(|e| format!("Failed to read lock file: {}", e))?;
    let lock: LockFile = serde_json::from_str(&content)
        .map_err(|e| format!("Lock file is corrupt: {}", e))?;

    let target_pid = lock.pid;
    let target_started_at = lock.started_at.clone();

    if is_pid_alive(target_pid) {
        // F-node-ui-003 (P2 fail-closed): re-read the lock file IMMEDIATELY
        // before SIGTERM. The original `lock` was read seconds ago for a
        // freshness probe; in the meantime the lock-holding process could
        // have exited and the OS could have recycled its PID for an
        // unrelated process. Comparing both `pid` AND `started_at` (the
        // ISO timestamp the original owner wrote) detects PID recycling
        // within the same lock-owner identity: a recycled PID will not
        // have rewritten the same `started_at`.
        match std::fs::read_to_string(&lock_path) {
            Ok(c1) => match serde_json::from_str::<LockFile>(&c1) {
                Ok(l1) if l1.pid == target_pid && l1.started_at == target_started_at => {
                    // Same owner — proceed with SIGTERM.
                }
                Ok(_) => {
                    return Err(format!(
                        "Lock file changed before termination (PID {} / startedAt {} no longer the owner); aborting to avoid killing an unrelated process.",
                        target_pid, target_started_at
                    ));
                }
                Err(e) => {
                    return Err(format!(
                        "Lock file became unreadable before termination: {}",
                        e
                    ));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Lock vanished — owner cleaned up between our two reads.
                // Skip the kill path; fall through to file removal (noop).
                return Ok(());
            }
            Err(e) => {
                return Err(format!(
                    "Lock file became unreadable before termination: {}",
                    e
                ));
            }
        }

        // Graceful SIGTERM first.
        let term_err = send_signal(target_pid, SIG_TERM);
        if let Err(e) = term_err {
            return Err(format!("Failed to terminate PID {}: {}", target_pid, e));
        }

        // Poll for graceful exit every 250 ms up to 5 s.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !is_pid_alive(target_pid) {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
        }

        // Re-verify the lock still points at our target (same pid AND
        // same started_at) before escalating to SIGKILL. PID recycle
        // protection: if the original process exited during the SIGTERM
        // grace window and the kernel handed the PID to a brand-new
        // process that happened to claim the lock, the `started_at`
        // field — written ONLY by the original owner — will not match.
        if is_pid_alive(target_pid) {
            match std::fs::read_to_string(&lock_path) {
                Ok(c2) => match serde_json::from_str::<LockFile>(&c2) {
                    Ok(l2) if l2.pid == target_pid && l2.started_at == target_started_at => {
                        // Same owner still — SIGKILL it.
                        send_signal(target_pid, SIG_KILL).map_err(|e| {
                            format!("Failed to SIGKILL PID {}: {}", target_pid, e)
                        })?;
                    }
                    Ok(_) => {
                        return Err(format!(
                            "Lock file changed during termination (PID {} / startedAt {} no longer the owner); aborting to avoid killing an unrelated process.",
                            target_pid, target_started_at
                        ));
                    }
                    Err(e) => {
                        return Err(format!("Lock file became unreadable mid-release: {}", e));
                    }
                },
                Err(_) => {
                    // Lock file vanished — the process cleaned itself up.
                    // Fall through to file removal (noop) and return Ok.
                }
            }
        }
    }

    // Remove the file. Tolerate ENOENT because the dying process may have
    // cleaned its own lock between the signal and now.
    match std::fs::remove_file(&lock_path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to remove lock file: {}", e)),
    }
}

/// Send a Unix signal to a PID. Returns Err with the OS errno message on
/// failure (typical: ESRCH = no such process, EPERM = not allowed).
#[cfg(unix)]
fn send_signal(pid: u32, signal: libc::c_int) -> Result<(), String> {
    // SAFETY: libc::kill is a syscall wrapper with well-defined behavior for
    // any pid_t / signal pair. No memory is dereferenced.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if rc == 0 {
        Ok(())
    } else {
        let err = std::io::Error::last_os_error();
        Err(err.to_string())
    }
}

#[cfg(not(unix))]
fn send_signal(_pid: u32, _signal: i32) -> Result<(), String> {
    Err("Signal sending not supported on this platform".to_string())
}

/// Validate a password by asking the node CLI to decrypt the wallet.
/// Returns structured success/failure so the UI can show proper errors.
#[tauri::command]
pub async fn unlock_wallet(app: AppHandle, password: String) -> Result<UnlockResult, String> {
    if password.is_empty() {
        return Ok(UnlockResult {
            success: false,
            wallet_address: None,
            error_code: Some("EMPTY_PASSWORD".to_string()),
            error_message: Some("Password is required.".to_string()),
        });
    }

    let mut cmd = build_node_command(Some(&app), &["wallet-verify"])?;
    cmd.env("SYNAPSEIA_WALLET_PASSWORD", &password);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(Stdio::null());

    let output = match tokio::time::timeout(CLI_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Ok(UnlockResult {
                success: false,
                wallet_address: None,
                error_code: Some("SPAWN_FAILED".to_string()),
                error_message: Some(format!(
                    "Failed to run node CLI. Is Node.js installed? {}",
                    e
                )),
            });
        }
        Err(_) => {
            return Ok(UnlockResult {
                success: false,
                wallet_address: None,
                error_code: Some("TIMEOUT".to_string()),
                error_message: Some(format!(
                    "wallet-verify timed out after {}s. Check Console.app for the node CLI log.",
                    CLI_TIMEOUT.as_secs()
                )),
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = strip_known_noise(&String::from_utf8_lossy(&output.stderr));
    let combined = format!("{}\n{}", stdout, stderr);
    eprintln!(
        "[synapseia-node-ui] unlock_wallet exit={:?} stdout={:?} stderr={:?}",
        output.status.code(),
        stdout.chars().take(400).collect::<String>(),
        stderr.chars().take(400).collect::<String>()
    );

    // Rust cannot trust the exit code alone because the node CLI bootstrap
    // logs noise on stderr; match the sentinel markers written by
    // wallet-verify for an unambiguous answer.
    if combined.contains("INVALID_PASSWORD") {
        return Ok(UnlockResult {
            success: false,
            wallet_address: None,
            error_code: Some("INVALID_PASSWORD".to_string()),
            error_message: Some("Incorrect password.".to_string()),
        });
    }
    if combined.contains("WALLET_NOT_FOUND") {
        return Ok(UnlockResult {
            success: false,
            wallet_address: None,
            error_code: Some("WALLET_NOT_FOUND".to_string()),
            error_message: Some("No wallet found. Create one first.".to_string()),
        });
    }

    if let Some(addr) = extract_pubkey(&combined) {
        return Ok(UnlockResult {
            success: true,
            wallet_address: Some(addr),
            error_code: None,
            error_message: None,
        });
    }

    Ok(UnlockResult {
        success: false,
        wallet_address: None,
        error_code: Some("UNKNOWN".to_string()),
        error_message: Some(format!(
            "Failed to verify wallet. stderr: {}",
            stderr.lines().last().unwrap_or("<empty>")
        )),
    })
}

#[tauri::command]
pub async fn create_wallet(
    app: AppHandle,
    password: String,
    node_name: String,
    model: Option<String>,
    llm_url: Option<String>,
    llm_key: Option<String>,
) -> Result<UnlockResult, String> {
    if password.len() < 8 {
        return Ok(UnlockResult {
            success: false,
            wallet_address: None,
            error_code: Some("PASSWORD_TOO_SHORT".to_string()),
            error_message: Some("Password must be at least 8 characters.".to_string()),
        });
    }
    if node_name.trim().is_empty() {
        return Ok(UnlockResult {
            success: false,
            wallet_address: None,
            error_code: Some("INVALID_NAME".to_string()),
            error_message: Some("Node name is required.".to_string()),
        });
    }

    // The coordinator URL is no longer user-configurable. The spawned node
    // CLI reads `COORDINATOR_URL` (or its hardcoded official fallback) at
    // startup; we inherit the parent process's env by default, so a UI
    // launched with `COORDINATOR_URL=...` propagates automatically.
    let mut args: Vec<String> = vec![
        "wallet-create".to_string(),
        "--name".to_string(),
        node_name,
    ];
    if let Some(m) = model {
        if !m.is_empty() {
            args.push("--model".to_string());
            args.push(m);
        }
    }
    if let Some(u) = llm_url {
        if !u.is_empty() {
            args.push("--llm-url".to_string());
            args.push(u);
        }
    }
    if let Some(k) = llm_key {
        if !k.is_empty() {
            args.push("--llm-key".to_string());
            args.push(k);
        }
    }

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut cmd = build_node_command(Some(&app), &arg_refs)?;
    cmd.env("SYNAPSEIA_WALLET_PASSWORD", &password);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(Stdio::null());

    let output = match tokio::time::timeout(CLI_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Ok(UnlockResult {
                success: false,
                wallet_address: None,
                error_code: Some("SPAWN_FAILED".to_string()),
                error_message: Some(format!(
                    "Failed to run node CLI. Is Node.js installed? {}",
                    e
                )),
            });
        }
        Err(_) => {
            return Ok(UnlockResult {
                success: false,
                wallet_address: None,
                error_code: Some("TIMEOUT".to_string()),
                error_message: Some(format!(
                    "wallet-create timed out after {}s.",
                    CLI_TIMEOUT.as_secs()
                )),
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = strip_known_noise(&String::from_utf8_lossy(&output.stderr));
    let combined = format!("{}\n{}", stdout, stderr);
    eprintln!(
        "[synapseia-node-ui] create_wallet exit={:?} stdout={:?} stderr={:?}",
        output.status.code(),
        stdout.chars().take(400).collect::<String>(),
        stderr.chars().take(400).collect::<String>()
    );

    if combined.contains("WALLET_ALREADY_EXISTS") {
        return Ok(UnlockResult {
            success: false,
            wallet_address: None,
            error_code: Some("WALLET_ALREADY_EXISTS".to_string()),
            error_message: Some(
                "A wallet already exists. Unlock with your existing password instead.".to_string(),
            ),
        });
    }
    if combined.contains("PASSWORD_TOO_SHORT") {
        return Ok(UnlockResult {
            success: false,
            wallet_address: None,
            error_code: Some("PASSWORD_TOO_SHORT".to_string()),
            error_message: Some("Password must be at least 8 characters.".to_string()),
        });
    }

    if let Some(addr) = extract_pubkey(&combined) {
        return Ok(UnlockResult {
            success: true,
            wallet_address: Some(addr),
            error_code: None,
            error_message: None,
        });
    }

    // Surface the actual node stderr so users see why it failed
    let last_err = stderr
        .lines()
        .rev()
        .find(|l| l.contains("ERROR") || l.contains("Error"))
        .unwrap_or("");
    Ok(UnlockResult {
        success: false,
        wallet_address: None,
        error_code: Some("CREATE_FAILED".to_string()),
        error_message: Some(if last_err.is_empty() {
            format!("Wallet creation failed. stderr: {}", stderr.trim())
        } else {
            last_err.to_string()
        }),
    })
}

// ─── Node Process Commands ────────────────────────────────────────────────────

#[tauri::command]
pub async fn start_node(
    password: String,
    app: AppHandle,
    state: State<'_, NodeProcessState>,
) -> Result<NodeStatus, String> {
    let node_state: NodeProcessState = (*state).clone();
    let mut node = node_state.lock().await;

    if node.child.is_some() {
        return Err("Node is already running".to_string());
    }

    // Refuse to double-start if the CLI (or another desktop instance) has
    // already claimed the lock on this machine.
    if let Some(info) = read_external_lock(None) {
        return Err(format!(
            "A Synapseia node is already running {} (PID {}). Stop it first.",
            if info.source.as_deref() == Some("ui") {
                "from another desktop session"
            } else {
                "from the CLI"
            },
            info.pid.unwrap_or(0)
        ));
    }

    let pid = spawn_node_into_state(&mut node, &app, &password, false)?;

    // Cache the password for the auto-respawn path. We only do this on the
    // operator-initiated start_node so a successful unlock here becomes the
    // basis for self-update restarts; stop_node clears it.
    // F-node-ui-005: wrap in Zeroizing so the bytes are scrubbed when the
    // field is later assigned to None or the struct drops.
    node.cached_password = Some(zeroize::Zeroizing::new(password));

    drop(node);

    Ok(NodeStatus {
        running: true,
        peer_id: None,
        tier: None,
        wallet: None,
        balance_sol: None,
        balance_syn: None,
        staked_syn: None,
        pid,
    })
}

/// Spawn the node CLI as a child of the UI process, plug stdout/stderr into
/// the existing log forwarders, and install the EOF watcher that handles
/// the `[SELF_UPDATE_RESTART]` auto-respawn flow.
///
/// Caller must hold the `NodeProcess` mutex guard. Returns the spawned PID
/// (None on systems where `Child::id()` is unavailable, e.g. after
/// `.wait()` — should not happen for a freshly spawned child).
///
/// `is_respawn` is purely informational for log output today; it lets the
/// emitted `node-self-update-restarted` event be distinguishable from a
/// fresh operator-initiated start.
fn spawn_node_into_state(
    node: &mut NodeProcess,
    app: &AppHandle,
    password: &str,
    is_respawn: bool,
) -> Result<Option<u32>, String> {
    // F-node-ui-004: rotate a fresh per-spawn nonce and inject it into
    // the child env. The child echoes it back in the canonical
    // `[SELF_UPDATE_RESTART]` marker; only that legitimate child knows
    // the nonce, so a malicious stdout line containing only the
    // substring can no longer trigger respawn.
    let nonce = generate_self_update_nonce();

    let mut cmd = build_node_command(Some(app), &["start"])?;
    cmd.env("SYNAPSEIA_WALLET_PASSWORD", password)
        .env("SYNAPSEIA_LAUNCH_SOURCE", "ui")
        .env("SYNAPSEIA_SELF_UPDATE_NONCE", &nonce)
        .env("NODE_ENV", "production")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Pin the agent-brain JSON to Tauri's per-OS app data dir so the node
    // child (which inherits cwd='/') doesn't try to mkdir /data. This is
    // the canonical fix; the node-side moduleDir-relative fallback in
    // agent-brain.ts is the safety net for non-Tauri spawns.
    if let Ok(data_dir) = app.path().app_data_dir() {
        if let Err(e) = std::fs::create_dir_all(&data_dir) {
            eprintln!(
                "[start_node] could not create app_data_dir {:?}: {} -- node will fall back to moduleDir-relative path",
                data_dir, e
            );
        } else {
            cmd.env("AGENT_BRAIN_PATH", data_dir.join("agent-brain.json"));
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn node process: {}", e))?;

    let pid = child.id();

    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();

    node.child = Some(child);
    node.pending_self_update_restart = false;
    node.generation = node.generation.wrapping_add(1);
    node.self_update_nonce = nonce.clone();
    let logs = node.logs.clone();
    let generation = node.generation;
    // Hand the per-spawn nonce to the stdout reader so it can strictly
    // validate any `[SELF_UPDATE_RESTART]` marker the child emits.
    let expected_nonce = nonce;

    if is_respawn {
        let _ = app.emit(
            "node-self-update-restarted",
            &serde_json::json!({ "pid": pid }),
        );
    }

    if let (Some(stdout), Some(stderr)) = (child_stdout, child_stderr) {
        let app_out = app.clone();
        let app_err = app.clone();
        let app_for_respawn = app.clone();
        let logs_out = logs.clone();
        let logs_err = logs.clone();
        let state_for_respawn: NodeProcessState =
            app.state::<NodeProcessState>().inner().clone();

        // stdout reader: forward log lines AND detect the [SELF_UPDATE_RESTART]
        // marker. On EOF (child closed stdout = process exiting/exited), if the
        // self-update flag is set we trigger the auto-respawn flow.
        //
        // F-node-ui-004: strict-anchor + per-spawn nonce. Only marker lines
        // emitted by the legitimate child (which knows the nonce via env)
        // can latch the flag.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(raw)) = reader.next_line().await {
                if parse_self_update_cue_with_nonce(&raw, &expected_nonce) {
                    let mut guard = state_for_respawn.lock().await;
                    // Only latch if the cue belongs to THE child we were spawned
                    // for. A delayed line from a previous generation must not
                    // flag a freshly started node for respawn.
                    if guard.generation == generation {
                        guard.pending_self_update_restart = true;
                    }
                    drop(guard);
                }
                let (level_override, message) = sanitise_log_line(&raw, "stdout");
                let log = LogLine {
                    timestamp: now_hhmmss(),
                    level: level_override,
                    message,
                };
                logs_out
                    .lock()
                    .await
                    .push(format!("[{}] {}", log.timestamp, log.message));
                let _ = app_out.emit("node-log", &log);
            }
            // EOF on stdout = the child process has closed its stdout, which
            // for our Node.js CLI happens at exit. Decide whether to auto-
            // respawn based on the flag latched above. We deliberately do
            // NOT try to differentiate exit-codes here: any clean exit that
            // emitted the cue is treated as a self-update relaunch request.
            handle_node_eof(state_for_respawn, app_for_respawn, generation).await;
        });

        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(raw)) = reader.next_line().await {
                let (level_override, message) = sanitise_log_line(&raw, "stderr");
                let log = LogLine {
                    timestamp: now_hhmmss(),
                    level: level_override,
                    message,
                };
                logs_err
                    .lock()
                    .await
                    .push(format!("[{}] [{}] {}", log.timestamp, log.level, log.message));
                let _ = app_err.emit("node-log", &log);
            }
        });
    }

    Ok(pid)
}

/// Called from the stdout reader when EOF is hit (child exited). Drives the
/// auto-respawn flow if the self-update cue was latched, otherwise leaves
/// state alone (operator sees the crash via the absent `running` flag on
/// the next status poll).
async fn handle_node_eof(state: NodeProcessState, app: AppHandle, my_generation: u64) {
    // Acquire the lock and decide what to do under it. Whatever we do must
    // be cheap — we hold the lock long enough for the respawn dispatch.
    let mut guard = state.lock().await;

    // Stale handler: a newer generation has already taken over (operator
    // clicked Start again after a manual stop, etc). Bail without touching
    // anything — the newer streamers own the state now.
    if guard.generation != my_generation {
        return;
    }

    let pending = guard.pending_self_update_restart;
    let pid = guard.child.as_ref().and_then(|c| c.id());

    // The child closed stdout — for our purposes it's gone. Drop the
    // handle so `node_status` reports running=false and any future
    // `start_node` from the UI can proceed.
    guard.child = None;
    guard.pending_self_update_restart = false;

    // Release any lockfile that still belongs to us. The CLI is supposed
    // to clean up its own lock, but a self-update exits before the
    // shutdown hooks run on some platforms, so this is the safety net.
    cleanup_lock_if_ours(pid);

    if !pending {
        // Genuine exit (crash or user-invoked stop). Preserve existing
        // behaviour: do nothing, the user sees the stopped state.
        return;
    }

    // Self-update path. We need a cached password to respawn — if it's
    // gone (operator locked the wallet between updates) surface an error
    // event so the UI can prompt for re-unlock.
    let Some(password) = guard.cached_password.clone() else {
        eprintln!(
            "[synapseia-node-ui] auto-respawn blocked: wallet locked, no cached password"
        );
        let _ = app.emit(
            "node-self-update-restart-failed",
            &serde_json::json!({
                "reason": "wallet-locked",
                "message": "Auto-restart blocked: wallet locked. Click Start to unlock and resume.",
            }),
        );
        return;
    };

    // Respawn. We're already holding the mutex, so call the helper
    // directly. Any failure surfaces as an event the UI can toast.
    // `password` is `Zeroizing<String>` (F-node-ui-005); explicit `&*…`
    // borrows the inner `&str` so we don't pass the smart-pointer type.
    // The Zeroizing wrapper here is dropped at end of this scope, which
    // scrubs the cloned bytes.
    match spawn_node_into_state(&mut guard, &app, &*password, true) {
        Ok(new_pid) => {
            eprintln!(
                "[synapseia-node-ui] auto-respawned after self-update: pid={:?}",
                new_pid
            );
        }
        Err(e) => {
            eprintln!(
                "[synapseia-node-ui] auto-respawn after self-update failed: {}",
                e
            );
            let _ = app.emit(
                "node-self-update-restart-failed",
                &serde_json::json!({
                    "reason": "spawn-failed",
                    "message": e,
                }),
            );
        }
    }
}

#[tauri::command]
pub async fn stop_node(state: State<'_, NodeProcessState>) -> Result<bool, String> {
    let mut node = state.lock().await;
    // A manual stop must NOT auto-respawn even if the CLI happened to be
    // mid-self-update when the operator clicked Stop. Clear both the flag
    // and the cached password — the next start will re-prompt.
    node.pending_self_update_restart = false;
    node.cached_password = None;
    // Invalidate the per-spawn self-update nonce so any late stdout line
    // from the now-dying child can no longer latch a respawn (F-node-ui-004).
    node.self_update_nonce.clear();
    // Bump generation so any in-flight EOF watcher from this child can't
    // mistake itself for the current owner of state when it wakes up.
    node.generation = node.generation.wrapping_add(1);
    if let Some(mut child) = node.child.take() {
        child.kill().await.map_err(|e| e.to_string())?;
        cleanup_lock_if_ours(child.id());
        Ok(true)
    } else {
        Err("No node running".to_string())
    }
}

/// Synchronous reap of any spawned node child. Called from the Tauri exit
/// event handler where `.await` is not available — we use `start_kill()`
/// which dispatches SIGKILL without blocking.
pub fn reap_on_exit(state: &NodeProcessState) {
    // `blocking_lock` waits for any in-flight async task (log streaming,
    // status poll) to release the mutex before we proceed. This is safe in
    // a shutdown context because those tasks hold the lock for microseconds
    // at most, so we never block more than a few milliseconds. The old
    // `try_lock` silently skipped the kill when any task happened to hold
    // the lock, leaving the node process alive after the window closed.
    let mut guard = state.blocking_lock();
    // App is shutting down — no auto-respawn under any circumstances.
    guard.pending_self_update_restart = false;
    guard.cached_password = None;
    guard.self_update_nonce.clear();
    guard.generation = guard.generation.wrapping_add(1);
    if let Some(mut child) = guard.child.take() {
        let pid = child.id();

        // Give the Node.js process a brief window to shut down cleanly
        // (flush logs, close WS connections) before we force-kill it.
        #[cfg(unix)]
        if let Some(p) = pid {
            let _ = std::process::Command::new("kill")
                .arg("-TERM")
                .arg(p.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            // Wait up to 2 s for graceful exit before escalating to SIGKILL.
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if !is_pid_alive(p) {
                    eprintln!("[synapseia-node-ui] reap_on_exit: pid={} exited cleanly", p);
                    cleanup_lock_if_ours(pid);
                    return;
                }
            }
        }

        let _ = child.start_kill();
        eprintln!("[synapseia-node-ui] reap_on_exit: killed child pid={:?}", pid);
        cleanup_lock_if_ours(pid);
    }
}

/// Remove ~/.synapseia/node.lock only if it still belongs to the PID we
/// just reaped. Another process (CLI user) may have reclaimed the stale
/// lock between our write and exit — don't evict them.
fn cleanup_lock_if_ours(our_pid: Option<u32>) {
    let Some(pid) = our_pid else { return };
    let path = synapseia_home().join("node.lock");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(lock) = serde_json::from_str::<LockFile>(&content) else {
        return;
    };
    if lock.pid == pid {
        let _ = std::fs::remove_file(&path);
        eprintln!("[synapseia-node-ui] released lock at {}", path.display());
    }
}

#[tauri::command]
pub async fn node_status(state: State<'_, NodeProcessState>) -> Result<NodeStatus, String> {
    let node = state.lock().await;
    let running = node.child.is_some();
    let pid = node.child.as_ref().and_then(|c| c.id());

    Ok(NodeStatus {
        running,
        peer_id: None,
        tier: None,
        wallet: None,
        balance_sol: None,
        balance_syn: None,
        staked_syn: None,
        pid,
    })
}

#[tauri::command]
pub async fn get_node_logs(state: State<'_, NodeProcessState>) -> Result<Vec<String>, String> {
    let node = state.lock().await;
    let logs = node.logs.lock().await.clone();
    Ok(logs)
}

#[tauri::command]
pub async fn run_command(
    app: AppHandle,
    command: String,
    args: Vec<String>,
    password: Option<String>,
) -> Result<CommandResult, String> {
    // SECURITY (F-node-ui-001): hard allowlist + per-command argv shape
    // validation BEFORE we hand anything to `node`. Without this guard a
    // renderer-side XSS / compromised dep / devtools call could pass a
    // Node.js flag (`--inspect-brk=0.0.0.0:9229`) through `command` and
    // attach a remote debugger to a process holding the cached wallet
    // password in `SYNAPSEIA_WALLET_PASSWORD`. See ALLOWED_COMMANDS /
    // validate_command_args above.
    if let Err(msg) = validate_command_args(&command, &args) {
        return Ok(CommandResult {
            success: false,
            output: String::new(),
            error: Some(msg),
        });
    }

    let mut all_args: Vec<String> = vec![command.clone()];
    all_args.extend(args);
    let arg_refs: Vec<&str> = all_args.iter().map(|s| s.as_str()).collect();

    let mut cmd = build_node_command(Some(&app), &arg_refs)?;
    if let Some(ref pwd) = password {
        cmd.env("SYNAPSEIA_WALLET_PASSWORD", pwd);
    }
    cmd.env("NODE_ENV", "production");
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(Stdio::null());

    let effective_timeout = timeout_for(&command);
    let output = match tokio::time::timeout(effective_timeout, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Ok(CommandResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Failed to run node CLI ({}): {}. Is Node.js installed?",
                    command, e
                )),
            });
        }
        Err(_) => {
            return Ok(CommandResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Command `{}` timed out after {}s.",
                    command,
                    effective_timeout.as_secs()
                )),
            });
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = strip_known_noise(&String::from_utf8_lossy(&output.stderr));

    if output.status.success() {
        Ok(CommandResult {
            success: true,
            output: stdout,
            error: None,
        })
    } else {
        Ok(CommandResult {
            success: false,
            output: stdout,
            error: Some(stderr),
        })
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Build a tokio Command that invokes `node <dist/bootstrap.js> <args...>`
/// (or `dist/index.js` as a fallback for pre-bootstrap CLI installs) with:
///   - an augmented PATH (GUI apps on macOS don't inherit shell PATH)
///   - the located @synapseia-network/node script
///
/// `app` is optional so commands without an AppHandle in scope (chain-info,
/// wallet-verify, run_command) still work — but when provided, the bundled-
/// inside-resources CLI becomes available as the final safety-net fallback.
///
/// `dist/bootstrap.js` is preferred because it installs the
/// `bigint-buffer` console.warn / stderr filter BEFORE the heavy CLI bundle
/// (`dist/index.js`) starts loading transitive Solana deps. Spawning
/// `dist/index.js` directly bypasses the filter, which is what leaked the
/// "bigint: Failed to load bindings" warning into captured stderr on every
/// platform up to 0.8.39. Pre-bootstrap CLI tarballs (< 0.8.0) fall back to
/// `dist/index.js` so legacy installs keep working.
fn build_node_command(app: Option<&AppHandle>, args: &[&str]) -> Result<Command, String> {
    let node_path = find_synapseia_node(app)?;
    let bootstrap_path = format!("{}/dist/bootstrap.js", node_path);
    let script_path = if std::path::Path::new(&bootstrap_path).exists() {
        bootstrap_path
    } else {
        format!("{}/dist/index.js", node_path)
    };

    // Prefer system node, fall back to the bundled runtime under
    // ~/.synapseia/node/. install_synapseia_node downloads the latter when
    // the user has no system install, and we keep using it across launches.
    let node_bin = locate_node_binary()
        .or_else(|| {
            let bundled = bundled_node_bin_path();
            if bundled.exists() { Some(bundled) } else { None }
        })
        .ok_or_else(|| "Could not find the `node` binary. Install Node.js (>=18) — e.g. `brew install node` — then relaunch Synapseia Node.".to_string())?;

    let mut cmd = Command::new(&node_bin);
    cmd.arg(&script_path);
    for a in args {
        cmd.arg(a);
    }
    let path_env = if let Some(parent) = node_bin.parent() {
        format!("{}:{}", parent.to_string_lossy(), augmented_path())
    } else {
        augmented_path()
    };
    cmd.env("PATH", path_env);
    // Forward the operator-configured Ollama endpoint (set via the desktop
    // Settings panel) into every spawned CLI invocation. Empty / unset =
    // CLI falls through to its hardcoded default (http://localhost:11434).
    // The CLI reads `OLLAMA_URL` across embeddings, hardware probe, ollama
    // chat, and training-llm.
    let settings = read_ui_settings_raw();
    let ollama_url = settings.ollama_url.trim();
    if !ollama_url.is_empty() {
        cmd.env("OLLAMA_URL", ollama_url);
    }
    // Cloud LLM env vars. The node CLI reads LLM_PROVIDER=cloud +
    // LLM_CLOUD_PROVIDER + LLM_CLOUD_MODEL + <PROVIDER>_API_KEY to wire
    // the selected provider into its inference adapters. If the operator
    // picked Ollama (or never picked a provider), we skip these and the
    // CLI falls back to local inference. This is the fix for the Windows
    // "settings revert to Ollama" bug: previously the Tauri shell never
    // forwarded the provider/key, so every spawn looked like a local-only
    // run regardless of what the UI had saved.
    if let Some(env_pairs) = cloud_llm_env_for(&settings) {
        for (k, v) in env_pairs {
            cmd.env(k, v);
        }
    }
    Ok(cmd)
}

/// Find a `node` binary even when the app was launched from Finder (where
/// PATH is usually just `/usr/bin:/bin:/usr/sbin:/sbin`).
fn locate_node_binary() -> Option<PathBuf> {
    // 1. respect an explicit override
    if let Ok(p) = std::env::var("SYNAPSEIA_NODE_BIN") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }

    // 2. check well-known install locations (ordered by likelihood on macOS)
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin/node"),    // Apple Silicon brew
        PathBuf::from("/usr/local/bin/node"),       // Intel brew / generic
        PathBuf::from("/usr/bin/node"),
        PathBuf::from("/snap/bin/node"),            // Linux snap
    ];

    // nvm installs live under ~/.nvm/versions/node/<ver>/bin/node — probe the
    // `current` symlink first, then the latest numeric version we can see.
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".nvm/current/bin/node"));
        let nvm_root = home.join(".nvm/versions/node");
        if let Ok(entries) = std::fs::read_dir(&nvm_root) {
            let mut versions: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|d| d.path()))
                .filter(|p| p.is_dir())
                .collect();
            // Sort descending so the newest version wins
            versions.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
            for v in versions {
                candidates.push(v.join("bin/node"));
            }
        }
        // Volta
        candidates.push(home.join(".volta/bin/node"));
        // fnm default shim
        candidates.push(home.join(".local/share/fnm/aliases/default/bin/node"));
    }

    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }

    // 3. last resort — use whatever the augmented PATH resolves to
    which_in_path("node")
}

fn which_in_path(binary: &str) -> Option<PathBuf> {
    let path = augmented_path();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(dir).join(binary);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Find an `npm` binary. We prefer the one sibling to the located node binary
/// because nvm/volta/fnm ship matched pairs and PATH alone can resolve to a
/// different toolchain than the one we're about to spawn `node` from.
fn locate_npm_binary() -> Option<PathBuf> {
    let npm_name = if cfg!(windows) { "npm.cmd" } else { "npm" };

    if let Some(node_bin) = locate_node_binary() {
        if let Some(bin_dir) = node_bin.parent() {
            let sibling = bin_dir.join(npm_name);
            if sibling.exists() {
                return Some(sibling);
            }
        }
    }

    which_in_path(npm_name)
}

#[derive(Serialize, Clone)]
struct InstallProgress {
    phase: &'static str,
    message: String,
}

/// Resolve the matched `npm` (or `npm.cmd` on Windows) sibling for a known
/// `node` binary. Used by the bundled-runtime install path where the system
/// `locate_npm_binary()` lookup would happily resolve to the wrong toolchain.
fn npm_for(node_bin: &std::path::Path) -> Option<PathBuf> {
    let npm_name = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let parent = node_bin.parent()?;
    let candidate = parent.join(npm_name);
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

/// Path to the bundled Node binary inside the per-user Synapseia home — the
/// layout we install into is platform-specific (Unix has `bin/node`, Windows
/// puts `node.exe` in the install root).
fn bundled_node_bin_path() -> PathBuf {
    let root = synapseia_home().join("node");
    if cfg!(windows) {
        root.join("node.exe")
    } else {
        root.join("bin/node")
    }
}

/// Resolve the URL + archive kind for the bundled Node tarball matching the
/// host platform. Returns None on architectures we don't ship for (FreeBSD,
/// 32-bit, etc.) — caller surfaces a clear error.
fn bundled_node_archive_url() -> Option<(String, &'static str)> {
    let v = BUNDLED_NODE_VERSION;
    let base = format!("https://nodejs.org/dist/v{v}");
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        Some((format!("{base}/node-v{v}-darwin-arm64.tar.gz"), "tar.gz"))
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        Some((format!("{base}/node-v{v}-darwin-x64.tar.gz"), "tar.gz"))
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some((format!("{base}/node-v{v}-linux-x64.tar.xz"), "tar.xz"))
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        Some((format!("{base}/node-v{v}-linux-arm64.tar.xz"), "tar.xz"))
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        Some((format!("{base}/node-v{v}-win-x64.zip"), "zip"))
    } else {
        None
    }
}

/// Returns a path to a working `node` binary. Tries (in order): system PATH +
/// well-known locations, the previously-bundled runtime under
/// `~/.synapseia/node/`, then a fresh download from nodejs.org. Emits
/// `install-progress` events so the UI can show download status.
async fn ensure_node_runtime(app: &AppHandle) -> Result<PathBuf, String> {
    // Acquire BEFORE the system-locate fast path so two concurrent callers
    // don't race fs probes and concurrent bundled-node downloads.
    let _guard = node_runtime_lock().lock().await;

    if let Some(p) = locate_node_binary() {
        return Ok(p);
    }

    let bundled = bundled_node_bin_path();
    if bundled.exists() {
        return Ok(bundled);
    }

    let (url, kind) = bundled_node_archive_url().ok_or_else(|| {
        "No prebuilt Node.js runtime is available for this platform/architecture. Install Node.js 20+ from https://nodejs.org/ manually."
            .to_string()
    })?;

    let _ = app.emit(
        "install-progress",
        InstallProgress {
            phase: "downloading-node",
            message: format!(
                "Downloading Node.js v{} (~30 MB)...",
                BUNDLED_NODE_VERSION
            ),
        },
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("Node download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Node download HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Node download body read failed: {e}"))?;

    // Official archive filename, exactly as it appears in SHASUMS256.txt.
    // bundled_node_archive_url() always returns a URL ending in this name.
    let archive_basename = url
        .rsplit('/')
        .next()
        .ok_or_else(|| format!("Could not derive archive filename from URL {url}"))?
        .to_string();

    // Fetch the official checksum manifest. Hard-fail on network blip — we
    // never extract an unverified tarball.
    let shasums_url = format!(
        "https://nodejs.org/dist/v{}/SHASUMS256.txt",
        BUNDLED_NODE_VERSION
    );
    let shasums_resp = reqwest::get(&shasums_url)
        .await
        .map_err(|e| format!("SHASUMS256 download failed: {e}"))?;
    if !shasums_resp.status().is_success() {
        return Err(format!("SHASUMS256 HTTP {}", shasums_resp.status()));
    }
    let shasums_text = shasums_resp
        .text()
        .await
        .map_err(|e| format!("SHASUMS256 body read failed: {e}"))?;
    let expected_hash = shasums_text
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?;
            if name == archive_basename {
                Some(hash.to_ascii_lowercase())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            format!(
                "SHASUMS256.txt did not list {archive_basename} for v{}",
                BUNDLED_NODE_VERSION
            )
        })?;

    let actual_hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        format!("{:x}", hasher.finalize())
    };
    if actual_hash != expected_hash {
        return Err(format!(
            "SHA256 mismatch for {archive_basename}: expected {expected_hash}, got {actual_hash}"
        ));
    }

    let arch = std::env::consts::ARCH;
    let archive_name = format!(
        "synapseia-node-v{}-{}.{}",
        BUNDLED_NODE_VERSION, arch, kind
    );
    let archive_path = std::env::temp_dir().join(&archive_name);
    std::fs::write(&archive_path, &bytes)
        .map_err(|e| format!("Failed to write Node archive to temp dir: {e}"))?;

    // RAII cleanup so a failed extraction doesn't leak a ~30 MB tarball.
    struct ArchiveCleanup<'a>(&'a Path);
    impl Drop for ArchiveCleanup<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _archive_cleanup = ArchiveCleanup(&archive_path);

    // Stage under synapseia_home() so the final rename(staging/inner ->
    // ~/.synapseia/node) is always intra-fs and atomic. /tmp can live on
    // a different filesystem (tmpfs vs ext4) on Linux.
    let synapseia_root = synapseia_home();
    std::fs::create_dir_all(&synapseia_root)
        .map_err(|e| format!("Failed to create synapseia home: {e}"))?;
    let staging = synapseia_root.join(format!(
        "node-staging-v{}-{}",
        BUNDLED_NODE_VERSION, arch
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("Failed to create staging dir: {e}"))?;

    let archive_for_extract = archive_path.clone();
    let staging_for_extract = staging.clone();
    let kind_str = kind.to_string();
    let extract_result: Result<(), String> = tokio::task::spawn_blocking(move || {
        // tar handles gzip + xz natively on macOS/Linux; Windows 10+ ships
        // bsdtar which also reads zip archives. PowerShell Expand-Archive is
        // the fallback if tar isn't on PATH for some reason.
        let status = std::process::Command::new("tar")
            .arg("-xf")
            .arg(&archive_for_extract)
            .arg("-C")
            .arg(&staging_for_extract)
            .status()
            .map_err(|e| format!("tar invocation failed: {e}"))?;
        if status.success() {
            return Ok(());
        }
        if cfg!(windows) && kind_str == "zip" {
            let staging_str = staging_for_extract.to_string_lossy().into_owned();
            let archive_str = archive_for_extract.to_string_lossy().into_owned();
            let ps = format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                archive_str.replace('\'', "''"),
                staging_str.replace('\'', "''")
            );
            let status = std::process::Command::new("powershell")
                .arg("-NoProfile")
                .arg("-Command")
                .arg(&ps)
                .status()
                .map_err(|e| format!("PowerShell Expand-Archive failed: {e}"))?;
            if status.success() {
                return Ok(());
            }
        }
        Err("Archive extraction failed (tar/Expand-Archive)".to_string())
    })
    .await
    .map_err(|e| format!("Extraction task panicked: {e}"))?;
    extract_result?;

    // Official tarballs unpack to a single `node-vX.Y.Z-os-arch/` directory.
    // Strip that prefix so the install layout is stable across version bumps.
    let inner = std::fs::read_dir(&staging)
        .map_err(|e| format!("Failed to read staging dir: {e}"))?
        .filter_map(|e| e.ok().map(|d| d.path()))
        .find(|p| p.is_dir())
        .ok_or_else(|| "Extracted archive contained no top-level directory".to_string())?;

    let target_root = synapseia_root.join("node");
    if let Some(parent) = target_root.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create install parent dir: {e}"))?;
    }
    // If a previous half-installed runtime exists, remove it to keep the
    // rename atomic. We just verified bundled_node_bin_path() didn't exist
    // above, but a partial dir without bin/node could still be lurking.
    if target_root.exists() {
        std::fs::remove_dir_all(&target_root)
            .map_err(|e| format!("Failed to clear stale runtime dir: {e}"))?;
    }
    std::fs::rename(&inner, &target_root).map_err(|e| {
        format!(
            "Failed to move extracted runtime into place: {e} (from {} to {})",
            inner.display(),
            target_root.display()
        )
    })?;

    let _ = std::fs::remove_dir_all(&staging);

    // Best-effort macOS quarantine strip. Most files have no xattr; failures
    // are expected and ignored. /usr/bin/xattr ships with every macOS install.
    #[cfg(target_os = "macos")]
    {
        let target_str = target_root.to_string_lossy().to_string();
        let _ = tokio::task::spawn_blocking(move || {
            std::process::Command::new("xattr")
                .arg("-dr")
                .arg("com.apple.quarantine")
                .arg(&target_str)
                .output()
        })
        .await;
    }

    let final_bin = bundled_node_bin_path();
    if !final_bin.exists() {
        return Err(format!(
            "Node runtime install completed but binary not found at {}",
            final_bin.display()
        ));
    }

    let _ = app.emit(
        "install-progress",
        InstallProgress {
            phase: "node-ready",
            message: format!("Node.js v{} installed", BUNDLED_NODE_VERSION),
        },
    );

    Ok(final_bin)
}


fn augmented_path() -> String {
    let base = std::env::var("PATH").unwrap_or_default();
    let mut extras = vec![
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ];
    if let Some(home) = dirs::home_dir() {
        extras.push(home.join(".volta/bin").to_string_lossy().to_string());
        extras.push(home.join(".nvm/current/bin").to_string_lossy().to_string());
        // Add the newest nvm version bin/
        let nvm_root = home.join(".nvm/versions/node");
        if let Ok(entries) = std::fs::read_dir(&nvm_root) {
            let mut versions: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|d| d.path()))
                .filter(|p| p.is_dir())
                .collect();
            versions.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
            if let Some(v) = versions.first() {
                extras.push(v.join("bin").to_string_lossy().to_string());
            }
        }
    }
    if base.is_empty() {
        extras.join(":")
    } else {
        format!("{}:{}", extras.join(":"), base)
    }
}

fn find_synapseia_node(_app: Option<&AppHandle>) -> Result<String, String> {
    // Each positive branch requires BOTH dist/index.js AND package.json to
    // exist — npm writes files in stages, so a mid-flight install can leave
    // dist/index.js without package.json (or vice-versa). Treating either as
    // a partial-state miss prevents callers from spawning against a half-
    // populated install while another invocation is still running `npm i -g`.

    // 1. explicit override (debug builds only).
    //    Honoring SYNAPSEIA_NODE_PATH in release builds is a privilege-escalation
    //    surface: any process able to write the user shell rc could redirect the
    //    locator to a hostile dist/index.js. The dev override is useful when
    //    iterating from the monorepo, so we keep it gated on debug_assertions.
    //
    //    F-node-ui-002 (P7): even in debug, gate the override by the same
    //    `"name": "@synapseia-network/node"` package.json substring check the
    //    other branches use — otherwise a stale shell rc pointing at an
    //    unrelated `dist/index.js` (e.g. a sibling tool laid out the same way)
    //    would silently spawn the wrong CLI with the wallet password env-var
    //    injected.
    #[cfg(debug_assertions)]
    {
        if let Ok(p) = std::env::var("SYNAPSEIA_NODE_PATH") {
            let pb = PathBuf::from(&p);
            let dist_ok = pb.join("dist/index.js").exists();
            let pkg_ok = pb.join("package.json").exists();
            if dist_ok && pkg_ok {
                if let Ok(pkg_text) = std::fs::read_to_string(pb.join("package.json")) {
                    if pkg_text.contains("\"name\": \"@synapseia-network/node\"")
                        || pkg_text.contains("\"name\":\"@synapseia-network/node\"")
                    {
                        return Ok(p);
                    }
                }
            }
        }
    }

    // 2. dev layout: src-tauri/../../.. up to packages/node (CARGO_MANIFEST_DIR
    //    is baked in at compile-time; only valid while running from the dev
    //    tree, but we still try it first because it's the cheapest check).
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(packages_dir) = manifest.parent().and_then(|p| p.parent()) {
        let candidate = packages_dir.join("node");
        let dist_ok = candidate.join("dist/index.js").exists();
        let pkg_ok = candidate.join("package.json").exists();
        if dist_ok && pkg_ok {
            return Ok(candidate.to_string_lossy().to_string());
        }
    }

    // 3. user-prefix npm-global install (HIGHEST priority after dev path):
    //    self-update writes `npm install -g` into ~/.synapseia/npm-global/
    //    via NPM_CONFIG_PREFIX so it never needs sudo. This MUST sit
    //    before the homebrew/global probe because users who originally
    //    installed via `npm install -g` (system prefix) leave that copy
    //    behind on disk; without preferring the user-prefix install here
    //    the relaunch after self-update keeps picking up the stale
    //    system copy and the version bump appears not to take effect.
    if let Some(home) = dirs::home_dir() {
        let user_prefix =
            home.join(".synapseia/npm-global/lib/node_modules/@synapseia-network/node");
        let dist_ok = user_prefix.join("dist/index.js").exists();
        let pkg_ok = user_prefix.join("package.json").exists();
        if dist_ok && pkg_ok {
            if let Ok(pkg_text) = std::fs::read_to_string(user_prefix.join("package.json")) {
                if pkg_text.contains("\"name\": \"@synapseia-network/node\"")
                    || pkg_text.contains("\"name\":\"@synapseia-network/node\"")
                {
                    return Ok(user_prefix.to_string_lossy().to_string());
                }
            }
        }
    }

    // 3b. bundled runtime install: when the user has no system Node, the app
    //     downloads Node into ~/.synapseia/node/ and `npm install -g` lands
    //     under <prefix>/lib/node_modules/.
    if let Some(home) = dirs::home_dir() {
        let bundled = home.join(".synapseia/node/lib/node_modules/@synapseia-network/node");
        let dist_ok = bundled.join("dist/index.js").exists();
        let pkg_ok = bundled.join("package.json").exists();
        if dist_ok && pkg_ok {
            if let Ok(pkg_text) = std::fs::read_to_string(bundled.join("package.json")) {
                if pkg_text.contains("\"name\": \"@synapseia-network/node\"")
                    || pkg_text.contains("\"name\":\"@synapseia-network/node\"")
                {
                    return Ok(bundled.to_string_lossy().to_string());
                }
            }
        }
    }

    // 3c. global npm installs (common dev machine) — homebrew / sudo-installed.
    //     LOWEST priority among the 3-tier homedir/system checks: if the user
    //     has BOTH a user-prefix install (3) AND a stale system-prefix copy,
    //     the user-prefix one wins.
    let npm_roots = [
        "/opt/homebrew/lib/node_modules",
        "/usr/local/lib/node_modules",
    ];
    for root in npm_roots {
        let c = format!("{}/@synapseia-network/node", root);
        let dist_ok = std::path::Path::new(&format!("{}/dist/index.js", c)).exists();
        let pkg_ok = std::path::Path::new(&format!("{}/package.json", c)).exists();
        if dist_ok && pkg_ok {
            // Defense-in-depth: confirm the package.json identifies as
            // @synapseia-network/node before trusting the path. Substring check
            // (no serde_json) — covers both `"name": "..."` and `"name":"..."`.
            if let Ok(pkg_text) = std::fs::read_to_string(format!("{}/package.json", c)) {
                if pkg_text.contains("\"name\": \"@synapseia-network/node\"")
                    || pkg_text.contains("\"name\":\"@synapseia-network/node\"")
                {
                    return Ok(c);
                }
            }
        }
    }

    // 4. ask npm itself where it puts global packages. Covers nvm/volta/fnm
    //    layouts we don't hard-code above. ~1-2 s cold-start cost is acceptable
    //    because this only runs after the cheaper checks miss.
    if let Some(npm_bin) = locate_npm_binary() {
        let mut cmd = std::process::Command::new(&npm_bin);
        cmd.arg("root").arg("-g").env("PATH", augmented_path());
        if let Ok(out) = cmd.output() {
            if out.status.success() {
                let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !root.is_empty() {
                    let c = format!("{}/@synapseia-network/node", root);
                    let dist_ok = std::path::Path::new(&format!("{}/dist/index.js", c)).exists();
                    let pkg_ok = std::path::Path::new(&format!("{}/package.json", c)).exists();
                    if dist_ok && pkg_ok {
                        // Defense-in-depth: an attacker who can swap the npm
                        // binary or hijack PATH could yield a bogus root.
                        // Confirm the discovered package.json identifies as
                        // @synapseia-network/node before returning.
                        if let Ok(pkg_text) =
                            std::fs::read_to_string(format!("{}/package.json", c))
                        {
                            if pkg_text.contains("\"name\": \"@synapseia-network/node\"")
                                || pkg_text.contains("\"name\":\"@synapseia-network/node\"")
                            {
                                return Ok(c);
                            }
                        }
                    }
                }
            }
        }
    }

    Err(format!(
        "{}: Could not locate @synapseia-network/node. Expected it at ../node relative to this binary, globally installed via npm, or installed under ~/.synapseia. Run install_synapseia_node to fetch it.",
        ERR_CLI_MISSING
    ))
}

/// Result of comparing the locally-installed @synapseia-network/node CLI
/// version against the npm registry's `latest` dist-tag plus the UI's own
/// compile-time floor (`MIN_NODE_CLI_VERSION`). Used by
/// `install_synapseia_node` to decide whether to short-circuit on a still-
/// fresh install or to fall through to the npm reinstall path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CliFreshness {
    UpToDate { current: String },
    Stale { current: String, latest: String },
}

#[tauri::command]
pub async fn install_synapseia_node(app: AppHandle) -> Result<String, String> {
    // Acquire BEFORE the early-return check so concurrent callers serialize.
    // The second waiter then re-runs find_synapseia_node() and observes the
    // completed install instead of kicking off a parallel `npm install -g`.
    let _guard = install_lock().lock().await;

    if let Ok(path) = find_synapseia_node(Some(&app)) {
        // Decide whether the existing install is current enough. On any error
        // from the version probe we fall back to the legacy
        // "already-installed" short-circuit so a transient coord 502 (or a
        // missing `node` binary, or a CLI that hangs on `--version`) never
        // blocks the desktop UI from booting.
        match check_cli_freshness(&app, &path).await {
            Ok(CliFreshness::UpToDate { current }) => {
                let _ = app.emit(
                    "install-progress",
                    InstallProgress {
                        phase: "already-installed",
                        message: format!("CLI already up-to-date (v{current})"),
                    },
                );
                return Ok(path);
            }
            Ok(CliFreshness::Stale { current, latest }) => {
                let _ = app.emit(
                    "install-progress",
                    InstallProgress {
                        phase: "upgrading",
                        message: format!("CLI v{current} -> v{latest}, upgrading..."),
                    },
                );
                // Fall through to the npm install -g path below. The lock is
                // still held so a concurrent caller will observe the upgrade
                // on its post-wait `find_synapseia_node` re-check.
            }
            Err(err) => {
                eprintln!("[install] freshness check failed: {err}; keeping existing CLI");
                let _ = app.emit(
                    "install-progress",
                    InstallProgress {
                        phase: "already-installed",
                        message: "CLI present, freshness check skipped".to_string(),
                    },
                );
                return Ok(path);
            }
        }
    }

    let node_bin = match ensure_node_runtime(&app).await {
        Ok(p) => p,
        Err(e) => return Err(format!("Node runtime setup failed: {e}")),
    };

    let npm_bin = npm_for(&node_bin)
        .or_else(locate_npm_binary)
        .ok_or_else(|| {
            "npm not found alongside node binary. Reinstall Node.js to ensure npm is included."
                .to_string()
        })?;

    // Put the resolved node's directory first on PATH so npm post-install
    // scripts and lifecycle hooks resolve to the SAME node we just located
    // (matters for the bundled runtime case — the system PATH may not see it).
    let path_env = if let Some(parent) = node_bin.parent() {
        format!("{}:{}", parent.to_string_lossy(), augmented_path())
    } else {
        augmented_path()
    };

    let _ = app.emit(
        "install-progress",
        InstallProgress {
            phase: "starting",
            message: "Installing @synapseia-network/node from npm registry...".to_string(),
        },
    );

    // Best-effort uninstall of the legacy `@synapseia/node` (pre-rename)
    // package. The two bins (`synapseia`, `syn`) collide on the new install
    // path with EEXIST when an older global install lingers from before the
    // npm scope rename. Errors here are ignored on purpose — the most
    // common case is "package not installed" which is fine.
    let npm_bin_uninstall = npm_bin.clone();
    let path_env_uninstall = path_env.clone();
    let _ = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&npm_bin_uninstall)
            .arg("uninstall")
            .arg("-g")
            .arg("@synapseia/node")
            .env("PATH", path_env_uninstall)
            .output()
    })
    .await;

    // Force `npm install -g` to land in the SAME user-prefix that
    // `find_synapseia_node` searches first (`~/.synapseia/npm-global`).
    // Without `NPM_CONFIG_PREFIX`, the install goes to the bundled-Node
    // default prefix (`~/.synapseia/node/lib/node_modules/...`) while
    // the locator still prefers the user-prefix copy. When an older CLI
    // already lives in user-prefix from a prior `npm install -g` or from
    // the CLI's own self-update, the next start spawns the stale user-prefix
    // copy, hits `preflightVersionCheck`, and triggers ANOTHER self-update
    // — producing the double-update loop the operator saw at 0.8.53 -> 0.8.54.
    let user_prefix = dirs::home_dir()
        .map(|h| h.join(".synapseia/npm-global"))
        .ok_or_else(|| "Could not resolve home dir".to_string())?;
    let _ = std::fs::create_dir_all(&user_prefix);
    let user_prefix_str = user_prefix.to_string_lossy().to_string();

    let install_result = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&npm_bin)
            .arg("install")
            .arg("-g")
            .arg("--force")
            .arg("@synapseia-network/node")
            .env("PATH", path_env)
            .env("NPM_CONFIG_PREFIX", user_prefix_str)
            .output()
    })
    .await
    .map_err(|e| format!("Failed to spawn npm install task: {}", e))?
    .map_err(|e| format!("Failed to launch npm: {}", e))?;

    if !install_result.status.success() {
        let stderr = String::from_utf8_lossy(&install_result.stderr);
        let stderr_lc = stderr.to_lowercase();
        if stderr_lc.contains("eacces") || stderr_lc.contains("permission denied") {
            return Err(
                "Permission denied installing global npm package. Try one of:\n 1. Use a Node version manager (nvm/volta/fnm) to avoid sudo.\n 2. Run 'sudo npm install -g @synapseia-network/node' in a terminal.\nSee: https://docs.npmjs.com/resolving-eacces-permissions-errors-when-installing-packages-globally"
                    .to_string(),
            );
        }
        let tail_start = stderr.len().saturating_sub(1000);
        return Err(format!(
            "npm install failed: {}",
            stderr[tail_start..].trim()
        ));
    }

    match find_synapseia_node(Some(&app)) {
        Ok(path) => {
            let _ = app.emit(
                "install-progress",
                InstallProgress {
                    phase: "complete",
                    message: "Installation complete".to_string(),
                },
            );
            Ok(path)
        }
        Err(_) => Err(
            "Install reported success but @synapseia-network/node not found. Try restarting the app."
                .to_string(),
        ),
    }
}

/// Phase event mirrored from the node CLI's `installPythonDeps` helper
/// (packages/node/src/utils/install-deps.ts). Field shape MUST match the
/// TypeScript `InstallDepsEvent` interface for the frontend subscriber to
/// destructure cleanly.
#[derive(Debug, Deserialize, Serialize, Clone)]
struct PythonInstallProgress {
    phase: String,
    status: String,
    message: String,
}

/// Run `syn install-deps` and stream phase events to the desktop UI.
///
/// Invoked by the loading screen so by the time the wallet unlock screen
/// appears, Python deps (venv + torch + LoRA stack + bitsandbytes) are
/// ready and clicking Start launches the node immediately without an
/// extra install delay.
///
/// Contract with the CLI: stdout lines prefixed with `[INSTALL_PROGRESS] `
/// carry a single-line JSON-encoded `PythonInstallProgress`. The remaining
/// stdout is forwarded as raw `node-log` events so operators can still see
/// pip output in the log panel. The CLI ALWAYS emits a final event with
/// `phase: "complete"`, but we also emit one ourselves on process exit so
/// the frontend can rely on receiving a terminal event even if the CLI
/// crashes before its own final emit.
#[tauri::command]
pub async fn install_python_deps(app: AppHandle) -> Result<String, String> {
    // Serialize concurrent invocations — frontend should only ever fire one,
    // but double-mount or rapid remount on the loading screen could
    // otherwise spawn two parallel pip-install runs.
    let _guard = python_install_lock().lock().await;

    let mut cmd = build_node_command(Some(&app), &["install-deps"])?;
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Non-interactive: the desktop spawn has no TTY. The CLI helper
        // already uses `stdio: 'pipe'` for pip subprocesses, so this just
        // hardens the contract.
        .env("CI", "1");

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn install-deps process: {}", e))?;

    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();

    if let Some(stdout) = child_stdout {
        let app_out = app.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(raw)) = reader.next_line().await {
                if let Some(rest) = raw.strip_prefix("[INSTALL_PROGRESS] ") {
                    match serde_json::from_str::<PythonInstallProgress>(rest) {
                        Ok(event) => {
                            let _ = app_out.emit("python-install-progress", &event);
                        }
                        Err(e) => {
                            eprintln!(
                                "[install_python_deps] failed to parse progress line: {} ({})",
                                e, rest
                            );
                        }
                    }
                } else {
                    // Forward non-progress stdout as a log line so operators
                    // can still see pip output in the log panel.
                    let log = LogLine {
                        timestamp: now_hhmmss(),
                        level: "info".to_string(),
                        message: raw,
                    };
                    let _ = app_out.emit("node-log", &log);
                }
            }
        });
    }

    if let Some(stderr) = child_stderr {
        let app_err = app.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(raw)) = reader.next_line().await {
                let log = LogLine {
                    timestamp: now_hhmmss(),
                    level: "warn".to_string(),
                    message: raw,
                };
                let _ = app_err.emit("node-log", &log);
            }
        });
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("install-deps process failed: {}", e))?;

    // Safety-net terminal event. The CLI helper already emits a `complete`
    // event itself, but if it crashes mid-flight (or before its first emit)
    // the frontend would hang on the loading screen waiting for a terminal
    // event that never arrives.
    if status.success() {
        let _ = app.emit(
            "python-install-progress",
            PythonInstallProgress {
                phase: "complete".to_string(),
                status: "done".to_string(),
                message: "Python deps install finished".to_string(),
            },
        );
        Ok("install-deps completed successfully".to_string())
    } else {
        let code = status.code().unwrap_or(-1);
        let msg = format!("install-deps exited with code {}", code);
        let _ = app.emit(
            "python-install-progress",
            PythonInstallProgress {
                phase: "complete".to_string(),
                status: "error".to_string(),
                message: msg.clone(),
            },
        );
        Err(msg)
    }
}

/// Best-effort npm-registry probe for the latest published version of
/// `@synapseia-network/node`. Returns the `latest` dist-tag on success.
/// Hard 5 s timeout — see `NPM_DIST_TAGS_TIMEOUT`. Any network / HTTP / parse
/// failure is folded into a single `Err(String)` so the caller can decide
/// whether to propagate or fall back to the `MIN_NODE_CLI_VERSION` floor.
async fn fetch_npm_latest() -> Result<String, String> {
    #[derive(Deserialize)]
    struct DistTags {
        latest: Option<String>,
    }

    let resp = reqwest::Client::new()
        .get(NPM_DIST_TAGS_URL)
        .timeout(NPM_DIST_TAGS_TIMEOUT)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("npm registry request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "npm registry returned status {}",
            resp.status()
        ));
    }

    let tags: DistTags = resp
        .json()
        .await
        .map_err(|e| format!("npm registry response was not JSON: {}", e))?;

    tags.latest
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "npm registry response had no `latest` dist-tag".to_string())
}

/// Return whichever of `a` / `b` is the larger semver. Ties (or parse
/// failures inside `semver_lt`) resolve to `a` — the caller always passes the
/// npm value as `a` and the floor as `b`, so a tie keeps the npm string for
/// downstream logging.
fn max_semver<'a>(a: &'a str, b: &'a str) -> &'a str {
    if semver_lt(a, b) { b } else { a }
}

/// Pure decision helper for `check_cli_freshness`. Splits the I/O (the npm
/// probe + the CLI `--version` probe) from the policy so the rules below are
/// unit-testable without spinning a real HTTP server.
///
/// Rules (in order — first match wins):
///   1. `current < min` → ALWAYS Stale. Target = `max(npm_latest, min)`.
///      Even if npm is down, we still know we need to upgrade because the
///      UI was shipped with a newer floor than what's installed.
///   2. npm reachable (`npm_latest = Some(v)`) AND `current < v` → Stale,
///      target = `v`. Standard "registry says upgrade" path.
///   3. npm reachable AND `current >= v` → UpToDate.
///   4. npm unreachable (`npm_latest = None`) AND `current >= min` → propagate
///      the npm error so the caller logs "freshness check skipped" and
///      keeps the existing CLI.
fn decide_cli_freshness(
    current: &str,
    npm_latest: Option<&str>,
    npm_error: Option<&str>,
    min: &str,
) -> Result<CliFreshness, String> {
    // Defense in depth (reviewer-lessons P2 — fail-closed cross-checks):
    // `semver_lt` silently treats non-numeric components as 0, so a malformed
    // `current` like "v0.8.49" or "broken" would compare as (0,0,0). Today
    // `extract_semver_line` rejects those upstream, but pinning the invariant
    // here makes the contract explicit and survives a future loosening of
    // that regex. The `min` side is always `MIN_NODE_CLI_VERSION`
    // (`env!("CARGO_PKG_VERSION")`) so a malformed value would be a
    // build-time bug — surface it the same way.
    if !is_semver(current) {
        return Err(format!("installed CLI reported malformed semver: {current:?}"));
    }
    if !is_semver(min) {
        return Err(format!("UI floor (MIN_NODE_CLI_VERSION) is malformed: {min:?}"));
    }

    // Rule 1: below the floor — always upgrade, regardless of npm reachability.
    if semver_lt(current, min) {
        let target = match npm_latest {
            Some(v) => max_semver(v, min).to_string(),
            None => min.to_string(),
        };
        return Ok(CliFreshness::Stale {
            current: current.to_string(),
            latest: target,
        });
    }

    match npm_latest {
        Some(v) => {
            // Rules 2 / 3: npm is the source of truth above the floor.
            if semver_lt(current, v) {
                Ok(CliFreshness::Stale {
                    current: current.to_string(),
                    latest: v.to_string(),
                })
            } else {
                Ok(CliFreshness::UpToDate {
                    current: current.to_string(),
                })
            }
        }
        None => {
            // Rule 4: above the floor and npm is down — honest "I don't know".
            Err(npm_error
                .map(|s| s.to_string())
                .unwrap_or_else(|| "npm registry unreachable".to_string()))
        }
    }
}

/// Decide whether the installed CLI at `node_path` is stale relative to the
/// latest published `@synapseia-network/node` AND the UI's own compile-time
/// floor (`MIN_NODE_CLI_VERSION`).
///
/// Two probes run sequentially:
///   1. `<node>/dist/bootstrap.js --version` (or `dist/index.js`) — the
///      installed CLI's reported version.
///   2. `https://registry.npmjs.org/-/package/@synapseia-network%2Fnode/dist-tags`
///      — the registry's `latest` dist-tag.
///
/// Decision policy lives in `decide_cli_freshness` (pure, unit-tested):
///   - Installed `<` UI floor → ALWAYS upgrade (defense in depth: registry
///     could be unreachable, rolled back, or temporarily lagging behind us).
///   - Installed `<` npm latest → upgrade to npm latest.
///   - Installed `>=` floor AND npm down → propagate Err so the caller logs
///     "freshness check skipped" and keeps the existing CLI.
///   - Otherwise → UpToDate.
///
/// Replaces the previous coord `/version` poll, which was fragile because
/// `latestNodeVersion` was baked into the coord's Docker image at build time
/// and could lag the npm publish by an entire `fly deploy` cycle.
async fn check_cli_freshness(
    _app: &AppHandle,
    node_path: &str,
) -> Result<CliFreshness, String> {
    // 1. Spawn the CLI's `--version` probe via the same node binary that
    //    `build_node_command` would use at runtime, so we exercise the same
    //    code path (matters for the bundled-runtime case where the system
    //    PATH may not see ~/.synapseia/node/bin/node).
    let node_bin = locate_node_binary()
        .or_else(|| {
            let bundled = bundled_node_bin_path();
            if bundled.exists() { Some(bundled) } else { None }
        })
        .ok_or_else(|| "node binary not found".to_string())?;
    let bootstrap_path = format!("{}/dist/bootstrap.js", node_path);
    let script_path = if std::path::Path::new(&bootstrap_path).exists() {
        bootstrap_path
    } else {
        format!("{}/dist/index.js", node_path)
    };

    let node_bin_owned = node_bin.clone();
    let script_owned = script_path.clone();
    let path_env = if let Some(parent) = node_bin.parent() {
        format!("{}:{}", parent.to_string_lossy(), augmented_path())
    } else {
        augmented_path()
    };
    let path_env_owned = path_env.clone();

    let version_join = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(&node_bin_owned)
                .arg(&script_owned)
                .arg("--version")
                .env("PATH", path_env_owned)
                .output()
        }),
    )
    .await
    .map_err(|_| "--version probe timed out after 30s".to_string())?
    .map_err(|e| format!("failed to join --version task: {e}"))?
    .map_err(|e| format!("failed to spawn --version probe: {e}"))?;

    if !version_join.status.success() {
        return Err(format!(
            "--version exit {:?}",
            version_join.status.code()
        ));
    }
    let stdout = String::from_utf8_lossy(&version_join.stdout);
    let current = extract_semver_line(&stdout).ok_or_else(|| {
        format!(
            "could not parse --version stdout: {}",
            stdout.chars().take(200).collect::<String>()
        )
    })?;

    // 2. Probe the npm registry's `latest` dist-tag. Failure here is folded
    //    into a structured `(None, Some(err))` tuple so the pure decision
    //    helper below can still force an upgrade when `current < MIN`.
    let (npm_latest, npm_error) = match fetch_npm_latest().await {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };

    // 3. Compare against npm + floor. See `decide_cli_freshness` for the
    //    full rule table.
    decide_cli_freshness(
        &current,
        npm_latest.as_deref(),
        npm_error.as_deref(),
        MIN_NODE_CLI_VERSION,
    )
}

/// Pick the last semver-looking line out of a CLI's `--version` stdout.
/// The bootstrap's logger emits two INFO lines before the version prints,
/// so the version is the LAST non-empty line that matches `^\d+\.\d+\.\d+$`
/// after ANSI escape stripping.
fn extract_semver_line(text: &str) -> Option<String> {
    text.lines()
        .map(|l| strip_ansi(l).trim().to_string())
        .filter(|l| is_semver(l))
        .last()
}

fn is_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Strict `<` comparison on dotted-numeric semver, ignoring any prerelease
/// / build metadata (the CLI only ships clean `X.Y.Z` tags).
fn semver_lt(a: &str, b: &str) -> bool {
    let to_tuple = |s: &str| -> (u64, u64, u64) {
        let p: Vec<u64> = s.split('.').filter_map(|x| x.parse().ok()).collect();
        (
            *p.first().unwrap_or(&0),
            *p.get(1).unwrap_or(&0),
            *p.get(2).unwrap_or(&0),
        )
    };
    to_tuple(a) < to_tuple(b)
}

/// Strip lines containing well-known transitive-dependency warnings from a
/// captured CLI stderr buffer. Currently filters:
///   - `bigint: Failed to load bindings, pure JS will be used` — emitted by
///     `bigint-buffer` (transitive dep of @solana/web3.js) when its native
///     binding can't load (the pure-JS fallback works correctly). The
///     CLI's bootstrap.ts already filters this in-process but on Windows
///     piped-stderr the in-process filter sometimes leaks.
fn strip_known_noise(buf: &str) -> String {
    buf.lines()
        .filter(|line| !line.contains("bigint: Failed to load bindings, pure JS will be used"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_pubkey(s: &str) -> Option<String> {
    // CLI emits `__WALLET_OK__ <pubkey>` on success. The sentinel is
    // intentionally distinct so grep/regex on logs can't yield a false match.
    const SENTINEL: &str = "__WALLET_OK__";
    for line in s.lines() {
        if let Some(rest) = line.find(SENTINEL) {
            let tail = &line[rest + SENTINEL.len()..].trim();
            let token = tail.split_whitespace().next().unwrap_or("");
            if is_base58_pubkey(token) {
                return Some(token.to_string());
            }
        }
    }
    None
}

fn is_base58_pubkey(s: &str) -> bool {
    let len = s.chars().count();
    if !(32..=44).contains(&len) {
        return false;
    }
    s.chars().all(|c| {
        matches!(
            c,
            '1'..='9'
                | 'A'..='H'
                | 'J'..='N'
                | 'P'..='Z'
                | 'a'..='k'
                | 'm'..='z'
        )
    })
}

/// The node CLI's logger prepends every line with ANSI colour codes plus a
/// human timestamp + level (`\x1b[90m01:50:27.568\x1b[0m  \x1b[32m\x1b[1mINFO\x1b[0m  message`).
/// The UI already stamps its own timestamp and level onto every line, so the
/// raw output ends up doubly-prefixed and full of unrendered escape codes.
/// Strip both the colours and the CLI's own prefix — return the clean
/// message and the canonical level detected from the prefix (falls back to
/// the pipe name, e.g. "stdout" / "stderr").
fn sanitise_log_line(raw: &str, default_level: &str) -> (String, String) {
    let no_ansi = strip_ansi(raw);
    let trimmed = no_ansi.trim_start();

    // Expect `HH:MM:SS[.mmm]  LEVEL  <rest>` at the start. Regex-free scan.
    let bytes = trimmed.as_bytes();
    if let Some((rest, level)) = scan_timestamp_level(bytes) {
        return (level.to_string(), rest.trim_start().to_string());
    }
    (default_level.to_string(), trimmed.to_string())
}

/// Remove every CSI escape sequence (`\x1b[...m` and related). No allocations
/// beyond the output string; fast enough for log-rate input.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Drop ESC, then skip until we hit a terminator byte (@-~ range)
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(nc) = chars.next() {
                    if (0x40..=0x7E).contains(&(nc as u32)) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Try to consume `HH:MM:SS(.mmm)?  LEVEL  ` from the start of `bytes`.
/// Returns (rest_of_line, lowercased_level) on success.
fn scan_timestamp_level(bytes: &[u8]) -> Option<(&str, String)> {
    if bytes.len() < 10 {
        return None;
    }
    let is_digit = |b: u8| b.is_ascii_digit();
    if !(is_digit(bytes[0]) && is_digit(bytes[1]) && bytes[2] == b':'
        && is_digit(bytes[3]) && is_digit(bytes[4]) && bytes[5] == b':'
        && is_digit(bytes[6]) && is_digit(bytes[7]))
    {
        return None;
    }
    let mut cursor = 8;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        while cursor < bytes.len() && is_digit(bytes[cursor]) {
            cursor += 1;
        }
    }
    let ws_start = cursor;
    while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
        cursor += 1;
    }
    if cursor == ws_start {
        return None;
    }
    let level_start = cursor;
    while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
        cursor += 1;
    }
    if cursor == level_start {
        return None;
    }
    let level = std::str::from_utf8(&bytes[level_start..cursor]).ok()?;
    if !matches!(level, "INFO" | "WARN" | "ERROR" | "DEBUG" | "TRACE" | "FATAL") {
        return None;
    }
    while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
        cursor += 1;
    }
    let rest = std::str::from_utf8(&bytes[cursor..]).ok()?;
    Some((rest, level.to_lowercase()))
}

fn now_hhmmss() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;
    let millis = now.subsec_millis();
    format!("{:02}:{:02}:{:02}.{:03}", hours, mins, secs, millis)
}

// ── Update checker ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub version: Option<String>,
    pub body: Option<String>,
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<UpdateInfo, String> {
    let updater = app
        .updater_builder()
        .build()
        .map_err(|e| format!("Updater init failed: {}", e))?;

    match updater.check().await {
        Ok(Some(update)) => Ok(UpdateInfo {
            available: true,
            version: Some(update.version.clone()),
            body: update.body.clone(),
        }),
        Ok(None) => Ok(UpdateInfo {
            available: false,
            version: None,
            body: None,
        }),
        Err(e) => {
            log::warn!("Update check failed: {}", e);
            Ok(UpdateInfo {
                available: false,
                version: None,
                body: None,
            })
        }
    }
}

// ── Molecular docking capability check ────────────────────────────────────

/// Whether the local machine has AutoDock Vina + Open Babel installed.
/// Surfaced in the desktop UI so users see WHY their node isn't picking
/// up MOLECULAR_DOCKING work orders, instead of silently being skipped.
/// Non-blocking — a missing binary just disables that workload type;
/// every other workload (research, training, inference) keeps working.
#[derive(Debug, serde::Serialize)]
pub struct DockingCapabilities {
    pub vina_available: bool,
    pub vina_path: Option<String>,
    pub vina_version: Option<String>,
    pub obabel_available: bool,
    pub obabel_path: Option<String>,
    pub obabel_version: Option<String>,
}

#[tauri::command]
pub async fn docking_capabilities() -> Result<DockingCapabilities, String> {
    use std::process::Command;

    let vina_path = which_in_path("vina");
    let vina_version = vina_path.as_ref().and_then(|p| {
        Command::new(p)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if stdout.is_empty() { None } else { Some(stdout) }
                } else { None }
            })
    });

    let obabel_path = which_in_path("obabel");
    let obabel_version = obabel_path.as_ref().and_then(|p| {
        Command::new(p)
            .arg("-V")
            .output()
            .ok()
            .and_then(|o| {
                // obabel writes the banner to stderr.
                let raw = if !o.stderr.is_empty() {
                    String::from_utf8_lossy(&o.stderr)
                } else {
                    String::from_utf8_lossy(&o.stdout)
                };
                let trimmed = raw.lines().next().unwrap_or("").trim().to_string();
                if trimmed.is_empty() { None } else { Some(trimmed) }
            })
    });

    Ok(DockingCapabilities {
        vina_available: vina_path.is_some(),
        vina_path: vina_path.as_ref().map(|p| p.display().to_string()),
        vina_version,
        obabel_available: obabel_path.is_some(),
        obabel_path: obabel_path.as_ref().map(|p| p.display().to_string()),
        obabel_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_settings_roundtrip_full() {
        // Full-shape roundtrip: every field present, JSON keys are camelCase
        // (matching the wire format used by invoke() from the React side).
        let original = UiSettings {
            ollama_url: "http://localhost:11435".to_string(),
            llm_provider: "nvidia".to_string(),
            llm_model_slug: "meta/llama-3.3-70b-instruct".to_string(),
            llm_api_key: "nvapi-test-key".to_string(),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        assert!(json.contains("\"ollamaUrl\""));
        assert!(json.contains("\"llmProvider\""));
        assert!(json.contains("\"llmModelSlug\""));
        assert!(json.contains("\"llmApiKey\""));
        let decoded: UiSettings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, original);
    }

    #[test]
    fn ui_settings_backward_compat_only_ollama_url() {
        // Legacy ui-settings.json files written by 0.8.35 and earlier only
        // contain the ollamaUrl field. The new fields must default to "" so
        // existing users do not see a deserialize crash on first launch
        // after the upgrade.
        let legacy_json = r#"{ "ollamaUrl": "http://10.0.0.5:11434" }"#;
        let decoded: UiSettings =
            serde_json::from_str(legacy_json).expect("legacy json must deserialize");
        assert_eq!(decoded.ollama_url, "http://10.0.0.5:11434");
        assert_eq!(decoded.llm_provider, "");
        assert_eq!(decoded.llm_model_slug, "");
        assert_eq!(decoded.llm_api_key, "");
    }

    #[test]
    fn ui_settings_backward_compat_empty_object() {
        // The absolute floor: a brand-new install with no fields at all.
        let decoded: UiSettings = serde_json::from_str("{}").expect("empty object");
        assert_eq!(decoded, UiSettings::default());
    }

    #[test]
    fn api_key_env_for_provider_known_providers() {
        // Every entry here must match the CLOUD_PROVIDERS_UI table in
        // packages/node-ui/src/lib/providers.ts. If the two drift, the
        // spawned CLI will be authenticated against the wrong env var.
        assert_eq!(api_key_env_for_provider("openai"), Some("OPENAI_API_KEY"));
        assert_eq!(
            api_key_env_for_provider("anthropic"),
            Some("ANTHROPIC_API_KEY")
        );
        assert_eq!(api_key_env_for_provider("google"), Some("GEMINI_API_KEY"));
        assert_eq!(
            api_key_env_for_provider("moonshot"),
            Some("MOONSHOT_API_KEY")
        );
        assert_eq!(
            api_key_env_for_provider("minimax"),
            Some("MINIMAX_API_KEY")
        );
        assert_eq!(api_key_env_for_provider("zhipu"), Some("ZHIPU_API_KEY"));
        assert_eq!(api_key_env_for_provider("nvidia"), Some("NVIDIA_API_KEY"));
    }

    #[test]
    fn api_key_env_for_provider_case_insensitive_and_trim() {
        // Operators may end up with mixed-case provider ids if a future
        // migration touches the field. Lowercasing here is defensive.
        assert_eq!(api_key_env_for_provider("NVIDIA"), Some("NVIDIA_API_KEY"));
        assert_eq!(api_key_env_for_provider(" openai "), Some("OPENAI_API_KEY"));
    }

    #[test]
    fn api_key_env_for_provider_unknown() {
        assert_eq!(api_key_env_for_provider("ollama"), None);
        assert_eq!(api_key_env_for_provider(""), None);
        assert_eq!(api_key_env_for_provider("custom"), None);
        assert_eq!(api_key_env_for_provider("openrouter"), None);
    }

    #[test]
    fn cloud_llm_env_skips_ollama_and_empty() {
        // Ollama selection: caller should NOT inject any cloud env vars.
        let ollama = UiSettings {
            ollama_url: "http://localhost:11434".to_string(),
            llm_provider: "ollama".to_string(),
            llm_model_slug: "qwen2.5:0.5b".to_string(),
            llm_api_key: "".to_string(),
        };
        assert!(cloud_llm_env_for(&ollama).is_none());

        // Empty provider: equivalent to "operator never picked anything".
        let empty_provider = UiSettings {
            llm_provider: "".to_string(),
            llm_model_slug: "nvidia/foo".to_string(),
            ..UiSettings::default()
        };
        assert!(cloud_llm_env_for(&empty_provider).is_none());

        // Empty model slug: incomplete selection, treat as not configured.
        let empty_slug = UiSettings {
            llm_provider: "nvidia".to_string(),
            llm_model_slug: "".to_string(),
            ..UiSettings::default()
        };
        assert!(cloud_llm_env_for(&empty_slug).is_none());
    }

    #[test]
    fn cloud_llm_env_for_nvidia_emits_expected_pairs() {
        let nvidia = UiSettings {
            ollama_url: "".to_string(),
            llm_provider: "nvidia".to_string(),
            llm_model_slug: "meta/llama-3.3-70b-instruct".to_string(),
            llm_api_key: "nvapi-abc".to_string(),
        };
        let env = cloud_llm_env_for(&nvidia).expect("nvidia selection must inject env");
        let map: std::collections::HashMap<String, String> = env.into_iter().collect();
        assert_eq!(map.get("LLM_PROVIDER").map(|s| s.as_str()), Some("cloud"));
        assert_eq!(
            map.get("LLM_CLOUD_PROVIDER").map(|s| s.as_str()),
            Some("nvidia")
        );
        assert_eq!(
            map.get("LLM_CLOUD_MODEL").map(|s| s.as_str()),
            Some("meta/llama-3.3-70b-instruct")
        );
        assert_eq!(
            map.get("NVIDIA_API_KEY").map(|s| s.as_str()),
            Some("nvapi-abc")
        );
    }

    #[test]
    fn cloud_llm_env_for_unknown_provider_skips_key_env() {
        // A future / unknown provider id still produces LLM_PROVIDER + the
        // cloud target so the CLI surfaces a clear "unknown provider"
        // error rather than silently dropping the selection. The API key
        // env is omitted because we have no name to bind it to.
        let unknown = UiSettings {
            ollama_url: "".to_string(),
            llm_provider: "future-provider".to_string(),
            llm_model_slug: "future-provider/some-model".to_string(),
            llm_api_key: "x".to_string(),
        };
        let env = cloud_llm_env_for(&unknown).expect("unknown provider still emits cloud env");
        let map: std::collections::HashMap<String, String> = env.into_iter().collect();
        assert_eq!(map.get("LLM_PROVIDER").map(|s| s.as_str()), Some("cloud"));
        assert_eq!(
            map.get("LLM_CLOUD_PROVIDER").map(|s| s.as_str()),
            Some("future-provider")
        );
        // No <PROVIDER>_API_KEY entry should be emitted.
        assert!(map.keys().all(|k| !k.ends_with("_API_KEY")));
    }
}

#[cfg(test)]
mod strip_known_noise_tests {
    use super::strip_known_noise;

    #[test]
    fn filters_bigint_warning_line() {
        let input = "bigint: Failed to load bindings, pure JS will be used (try npm run rebuild?)\n\
                     ❌ Invalid model format. Got: nvidia/meta/llama-3.3-70b-instruct";
        let out = strip_known_noise(input);
        assert!(!out.contains("bigint: Failed to load bindings"));
        assert!(out.contains("Invalid model format"));
    }

    #[test]
    fn preserves_unrelated_lines() {
        let input = "Some warning\nReal error message";
        assert_eq!(strip_known_noise(input), input);
    }

    #[test]
    fn handles_empty() {
        assert_eq!(strip_known_noise(""), "");
    }

    #[test]
    fn handles_only_noise() {
        let input = "bigint: Failed to load bindings, pure JS will be used\n\
                     bigint: Failed to load bindings, pure JS will be used (try npm run rebuild?)";
        assert_eq!(strip_known_noise(input), "");
    }
}

#[cfg(test)]
mod self_update_tests {
    //! Unit tests for the self-update relaunch detection. These tests are
    //! intentionally pure: they only exercise `parse_self_update_cue` plus
    //! the NodeProcess state transitions that the EOF handler would do on
    //! a real exit. We do NOT spawn real child processes — `tokio::process`
    //! is integration territory.

    use super::*;

    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn detects_canonical_marker_with_matching_nonce() {
        assert!(parse_self_update_cue_with_nonce(
            "[SELF_UPDATE_RESTART] nonce=0123456789abcdef0123456789abcdef v0.8.86 pid=12345",
            NONCE,
        ));
    }

    #[test]
    fn detects_marker_with_timestamp_prefix() {
        // Log forwarders / tee'd terminals can prepend a timestamp before
        // the marker line. The parser must still detect the cue.
        assert!(parse_self_update_cue_with_nonce(
            "2026-05-13T22:00:00.000Z [SELF_UPDATE_RESTART] nonce=0123456789abcdef0123456789abcdef v0.8.86 pid=999",
            NONCE,
        ));
    }

    #[test]
    fn rejects_marker_with_wrong_nonce() {
        // The substring is present but the nonce is unknown to an attacker
        // — must fail closed.
        assert!(!parse_self_update_cue_with_nonce(
            "[SELF_UPDATE_RESTART] nonce=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa v0.8.86 pid=1",
            NONCE,
        ));
    }

    #[test]
    fn rejects_marker_with_empty_expected_nonce() {
        // Empty expected_nonce — always rejected, even on a syntactically
        // valid marker line. Guards against the "no env, accept anything"
        // footgun.
        assert!(!parse_self_update_cue_with_nonce(
            "[SELF_UPDATE_RESTART] nonce=0123456789abcdef0123456789abcdef v0.8.86 pid=1",
            "",
        ));
        // The back-compat wrapper passes "" — always false.
        assert!(!parse_self_update_cue(
            "[SELF_UPDATE_RESTART] nonce=0123456789abcdef0123456789abcdef v0.8.86 pid=1"
        ));
    }

    #[test]
    fn rejects_legacy_substring_marker() {
        // Pre-F-node-ui-004 marker shape — `[SELF_UPDATE_RESTART] Update
        // applied, exiting for relaunch.` — must NOT match any longer.
        assert!(!parse_self_update_cue_with_nonce(
            "[SELF_UPDATE_RESTART] Update applied, exiting for relaunch.",
            NONCE,
        ));
    }

    #[test]
    fn rejects_marker_with_extra_tokens() {
        // Strict shape: exactly 3 tokens after the bracket. Anything else
        // — even with a valid nonce — fails closed.
        assert!(!parse_self_update_cue_with_nonce(
            "[SELF_UPDATE_RESTART] nonce=0123456789abcdef0123456789abcdef v0.8.86 pid=1 EXTRA",
            NONCE,
        ));
    }

    #[test]
    fn rejects_marker_with_bad_version() {
        assert!(!parse_self_update_cue_with_nonce(
            "[SELF_UPDATE_RESTART] nonce=0123456789abcdef0123456789abcdef vNOTSEMVER pid=1",
            NONCE,
        ));
    }

    #[test]
    fn rejects_marker_with_bad_pid() {
        assert!(!parse_self_update_cue_with_nonce(
            "[SELF_UPDATE_RESTART] nonce=0123456789abcdef0123456789abcdef v0.8.86 pid=notanumber",
            NONCE,
        ));
    }

    #[test]
    fn ignores_unrelated_lines() {
        assert!(!parse_self_update_cue_with_nonce("INFO node started", NONCE));
        assert!(!parse_self_update_cue_with_nonce("", NONCE));
        // Substring of the marker but missing the brackets should NOT match.
        assert!(!parse_self_update_cue_with_nonce(
            "SELF_UPDATE_RESTART (no brackets)",
            NONCE,
        ));
        // Different bracketed marker must not false-positive.
        assert!(!parse_self_update_cue_with_nonce(
            "[SELF_UPDATE_DOWNLOAD] something",
            NONCE,
        ));
    }

    // We can't pull in `#[tokio::test]` without adding the `macros` /
    // `rt-multi-thread` feature to tokio (forbidden by this sprint). Build
    // a current-thread runtime by hand from the `rt` feature we already
    // depend on and drive each scenario via `block_on`.
    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    }

    #[test]
    fn pending_flag_gates_respawn_decision() {
        // Models the contract used by handle_node_eof: the EOF watcher
        // only triggers respawn when the flag was latched by the stdout
        // reader. This test runs that decision purely on the state
        // struct without touching the Tauri app handle.
        rt().block_on(async {
            let proc = Arc::new(Mutex::new(NodeProcess::new()));

            {
                let mut g = proc.lock().await;
                assert!(!g.pending_self_update_restart);
                g.pending_self_update_restart = parse_self_update_cue_with_nonce(
                    "[SELF_UPDATE_RESTART] nonce=0123456789abcdef0123456789abcdef v0.8.86 pid=42",
                    "0123456789abcdef0123456789abcdef",
                );
            }

            let g = proc.lock().await;
            assert!(g.pending_self_update_restart);
        });
    }

    #[test]
    fn generation_bump_invalidates_stale_eof_handler() {
        // stop_node bumps the generation before clearing state. A stale
        // EOF watcher from the previously-running child must observe the
        // mismatch and bail without touching state.
        rt().block_on(async {
            let proc = Arc::new(Mutex::new(NodeProcess::new()));
            let captured_generation = {
                let mut g = proc.lock().await;
                g.generation = 5;
                g.generation
            };

            // Simulate stop_node bumping the generation.
            {
                let mut g = proc.lock().await;
                g.generation = g.generation.wrapping_add(1);
            }

            let g = proc.lock().await;
            assert_ne!(
                g.generation, captured_generation,
                "generation must change so stale EOF handler bails out"
            );
        });
    }

    #[test]
    fn stop_node_clears_cached_password_and_flag() {
        // Manual stop must wipe both fields so a later restart re-prompts
        // for the wallet password and never auto-respawns a stale process.
        rt().block_on(async {
            let proc = Arc::new(Mutex::new(NodeProcess::new()));
            {
                let mut g = proc.lock().await;
                g.cached_password = Some(zeroize::Zeroizing::new("secret".to_string()));
                g.pending_self_update_restart = true;
            }

            // Mimic stop_node's bookkeeping (without killing a real child).
            {
                let mut g = proc.lock().await;
                g.pending_self_update_restart = false;
                g.cached_password = None;
                g.generation = g.generation.wrapping_add(1);
            }

            let g = proc.lock().await;
            assert!(g.cached_password.is_none());
            assert!(!g.pending_self_update_restart);
        });
    }
}

#[cfg(test)]
mod cli_freshness_tests {
    //! Unit tests for the semver/ANSI helpers backing the freshness probe
    //! that `install_synapseia_node` runs before short-circuiting on an
    //! existing CLI install. The network/process-spawn branches are
    //! integration territory and not exercised here.
    use super::*;

    #[test]
    fn semver_lt_basic() {
        assert!(semver_lt("0.8.36", "0.8.42"));
        assert!(!semver_lt("0.8.42", "0.8.42"));
        assert!(!semver_lt("0.8.43", "0.8.42"));
    }

    #[test]
    fn semver_lt_minor_major() {
        assert!(semver_lt("0.7.99", "0.8.0"));
        assert!(semver_lt("0.9.99", "1.0.0"));
    }

    #[test]
    fn strip_ansi_basic() {
        assert_eq!(strip_ansi("\x1b[32mhello\x1b[0m"), "hello");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn extract_semver_picks_last_semver_line() {
        let stdout =
            "\x1b[90m10:00:00.000\x1b[0m  \x1b[32mINFO\x1b[0m  Booting\n0.8.42\n";
        assert_eq!(extract_semver_line(stdout), Some("0.8.42".to_string()));
    }

    #[test]
    fn extract_semver_rejects_non_semver() {
        assert_eq!(extract_semver_line("hello world"), None);
    }

    #[test]
    fn max_semver_picks_larger() {
        assert_eq!(max_semver("0.8.42", "0.8.50"), "0.8.50");
        assert_eq!(max_semver("0.8.50", "0.8.42"), "0.8.50");
        // Tie keeps `a` (the npm-supplied value at the call site).
        assert_eq!(max_semver("0.8.50", "0.8.50"), "0.8.50");
    }

    #[test]
    fn freshness_forces_upgrade_when_below_min_and_npm_down() {
        let res = decide_cli_freshness("0.8.49", None, Some("npm down"), "0.8.51").unwrap();
        assert_eq!(
            res,
            CliFreshness::Stale {
                current: "0.8.49".to_string(),
                latest: "0.8.51".to_string(),
            }
        );
    }

    #[test]
    fn freshness_forces_upgrade_when_below_min_and_npm_rolled_back() {
        // npm is reachable but reports a version older than the UI's floor
        // (operator rolled `latest` back via `npm dist-tag`). Floor still wins.
        let res = decide_cli_freshness("0.8.49", Some("0.8.50"), None, "0.8.51").unwrap();
        assert_eq!(
            res,
            CliFreshness::Stale {
                current: "0.8.49".to_string(),
                latest: "0.8.51".to_string(),
            }
        );
    }

    #[test]
    fn freshness_uses_npm_target_when_below_min_and_npm_ahead() {
        let res = decide_cli_freshness("0.8.49", Some("0.9.0"), None, "0.8.51").unwrap();
        assert_eq!(
            res,
            CliFreshness::Stale {
                current: "0.8.49".to_string(),
                latest: "0.9.0".to_string(),
            }
        );
    }

    #[test]
    fn freshness_up_to_date_when_at_min_and_npm_matches() {
        let res = decide_cli_freshness("0.8.51", Some("0.8.51"), None, "0.8.51").unwrap();
        assert_eq!(
            res,
            CliFreshness::UpToDate {
                current: "0.8.51".to_string()
            }
        );
    }

    #[test]
    fn freshness_stale_when_at_min_and_npm_ahead() {
        let res = decide_cli_freshness("0.8.51", Some("0.9.0"), None, "0.8.51").unwrap();
        assert_eq!(
            res,
            CliFreshness::Stale {
                current: "0.8.51".to_string(),
                latest: "0.9.0".to_string(),
            }
        );
    }

    #[test]
    fn freshness_propagates_err_when_above_min_and_npm_down() {
        let err =
            decide_cli_freshness("0.9.0", None, Some("registry timeout"), "0.8.51").unwrap_err();
        assert_eq!(err, "registry timeout");
    }

    #[test]
    fn freshness_up_to_date_when_above_min_and_npm_equal() {
        let res = decide_cli_freshness("0.9.0", Some("0.9.0"), None, "0.8.51").unwrap();
        assert_eq!(
            res,
            CliFreshness::UpToDate {
                current: "0.9.0".to_string()
            }
        );
    }

    #[test]
    fn freshness_below_min_with_npm_exactly_at_min() {
        // npm reachable and reports exactly the floor. Target must resolve to
        // the floor (tie via max_semver keeps the npm-supplied string, which
        // equals min) — never below.
        let res = decide_cli_freshness("0.8.49", Some("0.8.51"), None, "0.8.51").unwrap();
        assert_eq!(
            res,
            CliFreshness::Stale {
                current: "0.8.49".to_string(),
                latest: "0.8.51".to_string(),
            }
        );
    }

    #[test]
    fn freshness_rejects_malformed_current() {
        let err = decide_cli_freshness("v0.8.49", Some("0.8.51"), None, "0.8.51").unwrap_err();
        assert!(err.contains("malformed semver"), "unexpected error: {err}");
        // Also test fully non-numeric garbage.
        let err = decide_cli_freshness("nightly", None, None, "0.8.51").unwrap_err();
        assert!(err.contains("malformed semver"), "unexpected error: {err}");
    }

    #[test]
    fn freshness_rejects_malformed_min() {
        let err = decide_cli_freshness("0.8.49", Some("0.8.51"), None, "not-a-version").unwrap_err();
        assert!(err.contains("MIN_NODE_CLI_VERSION"), "unexpected error: {err}");
    }
}

#[cfg(test)]
mod external_lock_tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::TempDir;

    /// `SYNAPSEIA_HOME` is process-global env state — parallel tests would
    /// stomp on each other and silently read the wrong tmpdir's lock file.
    /// We serialize every external_lock_tests case behind a single mutex.
    /// The guard returned from `isolate_home` keeps the lock until end of
    /// scope (= end of test), so the next test waits.
    fn home_lock() -> MutexGuard<'static, ()> {
        static M: OnceLock<Mutex<()>> = OnceLock::new();
        // If a previous test panicked while holding the lock, the mutex gets
        // poisoned. We don't care — recover the inner data and continue.
        M.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Point SYNAPSEIA_HOME at a tmpdir for the duration of one test. The
    /// `_guard` keeps the tmpdir alive AND keeps the env-var mutex held;
    /// both drop at end of scope. Returns the lock path so the test can
    /// write/inspect it.
    fn isolate_home() -> (TempDir, PathBuf, MutexGuard<'static, ()>) {
        let guard = home_lock();
        let dir = TempDir::new().expect("tmpdir");
        std::env::set_var("SYNAPSEIA_HOME", dir.path());
        let lock = dir.path().join("node.lock");
        (dir, lock, guard)
    }

    fn write_lock(path: &Path, pid: u32, source: &str, started_at: &str) {
        let body = format!(
            r#"{{"pid":{},"startedAt":"{}","source":"{}"}}"#,
            pid, started_at, source
        );
        let mut f = std::fs::File::create(path).expect("create lock");
        f.write_all(body.as_bytes()).expect("write lock");
    }

    #[tokio::test]
    async fn check_external_lock_returns_none_when_no_file() {
        let (_dir, lock, _mutex) = isolate_home();
        assert!(!lock.exists());
        let result = check_external_lock().await;
        assert!(result.is_none(), "expected None for missing lock file");
    }

    #[tokio::test]
    async fn check_external_lock_reports_alive_for_current_process() {
        // Use our own PID — guaranteed alive for the duration of the test.
        let (_dir, lock, _mutex) = isolate_home();
        let our_pid = std::process::id();
        let started = chrono::Utc::now().to_rfc3339();
        write_lock(&lock, our_pid, "cli", &started);

        let info = check_external_lock().await.expect("expected Some");
        assert_eq!(info.pid, our_pid);
        assert_eq!(info.source, "cli");
        assert!(info.is_alive, "current process must be alive");
        // age_seconds is small but non-negative.
        assert!(info.age_seconds < 60, "age should be tiny");
    }

    #[tokio::test]
    async fn check_external_lock_reports_dead_for_unused_pid() {
        // PID 0 is reserved (kernel) on Unix; is_pid_alive short-circuits to
        // false. This exercises the dead-PID path without spawning + reaping a
        // real process (which is racy in CI).
        let (_dir, lock, _mutex) = isolate_home();
        let started = chrono::Utc::now().to_rfc3339();
        write_lock(&lock, 0, "cli", &started);

        let info = check_external_lock().await.expect("expected Some");
        assert_eq!(info.pid, 0);
        assert!(!info.is_alive, "PID 0 must report not-alive");
    }

    #[tokio::test]
    async fn check_external_lock_normalises_unknown_source() {
        let (_dir, lock, _mutex) = isolate_home();
        let our_pid = std::process::id();
        let started = chrono::Utc::now().to_rfc3339();
        write_lock(&lock, our_pid, "operator-future-tag", &started);

        let info = check_external_lock().await.expect("expected Some");
        assert_eq!(info.source, "unknown");
    }

    #[tokio::test]
    async fn force_release_lock_removes_file_when_no_process_to_kill() {
        let (_dir, lock, _mutex) = isolate_home();
        // PID 0 = dead, so force_release should skip the SIGTERM path entirely
        // and just delete the file.
        let started = chrono::Utc::now().to_rfc3339();
        write_lock(&lock, 0, "cli", &started);
        assert!(lock.exists());

        force_release_lock().await.expect("force release should succeed");
        assert!(!lock.exists(), "lock file should be removed");
    }

    #[tokio::test]
    async fn force_release_lock_is_idempotent_when_no_file() {
        let (_dir, lock, _mutex) = isolate_home();
        assert!(!lock.exists());
        // Calling against a missing file is a no-op success, not an error.
        force_release_lock().await.expect("missing file is Ok");
    }

    #[test]
    fn parse_iso8601_age_seconds_handles_valid_input() {
        // 10 seconds ago.
        let ts = (chrono::Utc::now() - chrono::Duration::seconds(10)).to_rfc3339();
        let age = parse_iso8601_age_seconds(&ts);
        assert!(age >= 9 && age <= 12, "age was {age}");
    }

    #[test]
    fn parse_iso8601_age_seconds_returns_zero_for_garbage() {
        assert_eq!(parse_iso8601_age_seconds("not-a-date"), 0);
        assert_eq!(parse_iso8601_age_seconds(""), 0);
    }
}
