mod db;
mod delete;
mod fingerprint;
mod host;
mod jobs;
mod match_dups;
mod media;
mod paths;
mod scan;
mod tools_dl;

use std::sync::Arc;

use jobs::JobHub;
use tauri::Manager;

#[tauri::command]
fn get_host_url() -> String {
    host::host_url()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = paths::init_from_cli_and_env();
    let _ = db::CatalogDb::open();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let hub = Arc::new(JobHub::default());
            app.manage(hub.clone());
            host::start(hub);

            if let Some(window) = app.get_webview_window("main") {
                let window = window.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        if host::bound_port().is_some() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    let url = host::host_url();
                    let _ = window.eval(&format!("window.location.replace('{url}')"));
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_host_url])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
