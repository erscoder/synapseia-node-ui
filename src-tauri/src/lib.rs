mod commands;

use commands::{
    create_wallet, external_node_info, fetch_chain_info, get_node_logs, node_status, reap_on_exit,
    run_command, start_node, stop_node, system_info, unlock_wallet, wallet_exists, NodeProcess,
    NodeProcessState,
};

use std::sync::Arc;
use tauri::{Manager, RunEvent};
use tokio::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let node_state: NodeProcessState = Arc::new(Mutex::new(NodeProcess::new()));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                ))
                .build(),
        )
        .manage(node_state)
        .invoke_handler(tauri::generate_handler![
            start_node,
            stop_node,
            node_status,
            get_node_logs,
            run_command,
            wallet_exists,
            unlock_wallet,
            create_wallet,
            external_node_info,
            fetch_chain_info,
            system_info,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // When the user closes the window (or the app receives SIGTERM), make
    // sure we don't orphan the spawned node child. Without this the child
    // keeps talking to the coordinator long after the UI is gone —
    // exactly the "268 zombie processes" situation we just cleaned up.
    app.run(|app_handle, event| match event {
        RunEvent::ExitRequested { .. } | RunEvent::Exit => {
            if let Some(state) = app_handle.try_state::<NodeProcessState>() {
                reap_on_exit(&state);
            }
        }
        _ => {}
    });
}
