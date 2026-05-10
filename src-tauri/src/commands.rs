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

pub struct NodeProcess {
    child: Option<Child>,
    logs: Arc<Mutex<Vec<String>>>,
}

impl NodeProcess {
    pub fn new() -> Self {
        Self {
            child: None,
            logs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

pub type NodeProcessState = Arc<Mutex<NodeProcess>>;

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
pub async fn fetch_chain_info() -> Result<ChainInfo, String> {
    let mut cmd = build_node_command(&["chain-info"])?;
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

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
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

fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // Rust std doesn't wrap kill(2) directly and we'd rather not pull in
    // `nix` just for this. `kill -0 <pid>` does an existence + permission
    // check without actually signalling, runs in <1ms, and is present on
    // every Unix we ship to (macOS, Linux).
    #[cfg(unix)]
    {
        let status = std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        return matches!(status, Ok(s) if s.success());
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Validate a password by asking the node CLI to decrypt the wallet.
/// Returns structured success/failure so the UI can show proper errors.
#[tauri::command]
pub async fn unlock_wallet(password: String) -> Result<UnlockResult, String> {
    if password.is_empty() {
        return Ok(UnlockResult {
            success: false,
            wallet_address: None,
            error_code: Some("EMPTY_PASSWORD".to_string()),
            error_message: Some("Password is required.".to_string()),
        });
    }

    let mut cmd = build_node_command(&["wallet-verify"])?;
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
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
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
    let mut cmd = build_node_command(&arg_refs)?;
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
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
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
    let mut node = state.lock().await;

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

    let mut cmd = build_node_command(&["start"])?;
    cmd.env("SYNAPSEIA_WALLET_PASSWORD", &password)
        .env("SYNAPSEIA_LAUNCH_SOURCE", "ui")
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
    let logs = node.logs.clone();

    drop(node);

    if let (Some(stdout), Some(stderr)) = (child_stdout, child_stderr) {
        let app_out = app.clone();
        let app_err = app.clone();
        let logs_out = logs.clone();
        let logs_err = logs.clone();

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(raw)) = reader.next_line().await {
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

#[tauri::command]
pub async fn stop_node(state: State<'_, NodeProcessState>) -> Result<bool, String> {
    let mut node = state.lock().await;
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
    command: String,
    args: Vec<String>,
    password: Option<String>,
) -> Result<CommandResult, String> {
    let mut all_args: Vec<String> = vec![command.clone()];
    all_args.extend(args);
    let arg_refs: Vec<&str> = all_args.iter().map(|s| s.as_str()).collect();

    let mut cmd = build_node_command(&arg_refs)?;
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
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

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

/// Build a tokio Command that invokes `node <dist/index.js> <args...>` with:
///   - an augmented PATH (GUI apps on macOS don't inherit shell PATH)
///   - the located @synapseia-network/node script
fn build_node_command(args: &[&str]) -> Result<Command, String> {
    let node_path = find_synapseia_node()?;
    let script_path = format!("{}/dist/index.js", node_path);

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

fn find_synapseia_node() -> Result<String, String> {
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
    #[cfg(debug_assertions)]
    {
        if let Ok(p) = std::env::var("SYNAPSEIA_NODE_PATH") {
            let pb = PathBuf::from(&p);
            let dist_ok = pb.join("dist/index.js").exists();
            let pkg_ok = pb.join("package.json").exists();
            if dist_ok && pkg_ok {
                return Ok(p);
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

    // 3. global npm installs (common dev machine) — try both possible node-binary
    //    roots for @synapseia-network/node.
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
        "{}: Could not locate @synapseia-network/node. Expected it at ../node relative to this binary or globally installed.",
        ERR_CLI_MISSING
    ))
}

#[tauri::command]
pub async fn install_synapseia_node(app: AppHandle) -> Result<String, String> {
    // Acquire BEFORE the early-return check so concurrent callers serialize.
    // The second waiter then re-runs find_synapseia_node() and observes the
    // completed install instead of kicking off a parallel `npm install -g`.
    let _guard = install_lock().lock().await;

    if let Ok(path) = find_synapseia_node() {
        let _ = app.emit(
            "install-progress",
            InstallProgress {
                phase: "already-installed",
                message: "@synapseia-network/node already installed".to_string(),
            },
        );
        return Ok(path);
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

    let install_result = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&npm_bin)
            .arg("install")
            .arg("-g")
            .arg("--force")
            .arg("@synapseia-network/node")
            .env("PATH", path_env)
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

    match find_synapseia_node() {
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
