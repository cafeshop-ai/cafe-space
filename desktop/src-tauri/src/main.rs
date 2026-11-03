#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod installation;
mod proxy;
use installation::{InstallationInfo, InstallationProgress, InstallationResult, Installer};
use proxy::{DaemonDelivery, Proxy};
use tauri::{State, ipc::Channel};

#[tauri::command]
async fn daemon_open(
    events: Channel<DaemonDelivery>,
    proxy: State<'_, Proxy>,
) -> Result<String, &'static str> {
    proxy.open(build_flavor(), events).await
}

#[tauri::command]
async fn daemon_send(
    connection_id: String,
    frame: String,
    proxy: State<'_, Proxy>,
) -> Result<(), &'static str> {
    proxy.send(&connection_id, &frame).await
}

#[tauri::command]
async fn daemon_ack(
    connection_id: String,
    sequence: u32,
    proxy: State<'_, Proxy>,
) -> Result<(), &'static str> {
    proxy.ack(&connection_id, sequence).await
}

#[tauri::command]
async fn daemon_close(connection_id: String, proxy: State<'_, Proxy>) -> Result<(), &'static str> {
    proxy.close(&connection_id).await
}

#[tauri::command]
async fn m0_finish(app: tauri::AppHandle, proxy: State<'_, Proxy>) -> Result<(), &'static str> {
    if !cfg!(debug_assertions) || std::env::var("CAFE_SPACE_M0_PROBE").as_deref() != Ok("1") {
        return Err("M0_PROBE_DISABLED");
    }
    if !proxy.confirmed_empty().await {
        return Err("M0_PROBE_NOT_CONFIRMED");
    }
    println!("M0_PROXY_WEBVIEW_OK");
    app.exit(0);
    Ok(())
}

fn build_flavor() -> cafe_space_ipc::BuildFlavor {
    if cfg!(debug_assertions) {
        cafe_space_ipc::BuildFlavor::Dev
    } else {
        cafe_space_ipc::BuildFlavor::Release
    }
}

#[tauri::command]
async fn daemon_installation_info(
    installer: State<'_, Installer>,
) -> Result<InstallationInfo, &'static str> {
    installer.info(build_flavor()).await
}
#[tauri::command]
async fn daemon_install(
    events: Channel<InstallationProgress>,
    installer: State<'_, Installer>,
) -> Result<InstallationResult, &'static str> {
    installer.install(build_flavor(), events).await
}
#[tauri::command]
async fn daemon_start(installer: State<'_, Installer>) -> Result<InstallationResult, &'static str> {
    installer.start(build_flavor()).await
}
#[tauri::command]
async fn daemon_update(
    mode: cafe_space_protocol::UpdateMode,
    events: Channel<InstallationProgress>,
    installer: State<'_, Installer>,
) -> Result<InstallationResult, &'static str> {
    installer.update(build_flavor(), mode, events).await
}
#[tauri::command]
async fn daemon_cancel_update(installer: State<'_, Installer>) -> Result<(), &'static str> {
    installer.cancel()
}
#[tauri::command]
async fn daemon_recover(
    events: Channel<InstallationProgress>,
    installer: State<'_, Installer>,
) -> Result<InstallationResult, &'static str> {
    installer.recover(build_flavor(), events).await
}

fn main() {
    #[cfg(debug_assertions)]
    let context = tauri::generate_context!("tauri.dev.conf.json");
    #[cfg(not(debug_assertions))]
    let context = tauri::generate_context!();
    tauri::Builder::default()
        .manage(Proxy::default())
        .manage(Installer::default())
        .invoke_handler(tauri::generate_handler![
            daemon_open,
            daemon_send,
            daemon_ack,
            daemon_close,
            daemon_installation_info,
            daemon_install,
            daemon_start,
            daemon_update,
            daemon_cancel_update,
            daemon_recover,
            m0_finish
        ])
        .run(context)
        .expect("desktop runtime");
}

#[cfg(test)]
mod tests {
    #[test]
    fn release_and_development_identity_are_isolated_without_configuration_drift() {
        let mut release: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let mut development: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.dev.conf.json")).unwrap();
        assert_eq!(release["identifier"], "ai.cafeshop.cafe-space");
        assert_eq!(development["identifier"], "ai.cafeshop.cafe-space-dev");
        release.as_object_mut().unwrap().remove("identifier");
        development.as_object_mut().unwrap().remove("identifier");
        assert_eq!(release, development);
    }
}
