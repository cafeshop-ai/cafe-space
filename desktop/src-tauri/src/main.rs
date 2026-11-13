#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod installation;
mod probe;
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

#[tauri::command]
async fn m1_finish(
    app: tauri::AppHandle,
    proxy: State<'_, Proxy>,
    success: bool,
) -> Result<(), &'static str> {
    if !probe::enabled() {
        return Err("M1_PROBE_DISABLED");
    }
    if success && proxy.confirmed_chat().await {
        println!("M1_HOST_WEBVIEW_OK");
        println!("M1_HOST_WEBVIEW_FINAL_ACK_OK");
        app.exit(0);
    } else {
        println!("M1_HOST_WEBVIEW_FAILED");
        app.exit(1);
    }
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
    action: cafe_space_protocol::UpdateRecoveryAction,
    events: Channel<InstallationProgress>,
    installer: State<'_, Installer>,
) -> Result<InstallationResult, &'static str> {
    installer.recover(build_flavor(), action, events).await
}

fn external_links() -> impl tauri::plugin::Plugin<tauri::Wry> {
    // Only the explicit confirmation action may open a link.
    tauri_plugin_opener::Builder::new()
        .open_js_links_on_click(false)
        .build()
}

fn context() -> tauri::Context<tauri::Wry> {
    // macOS embeds a global Info.plist symbol; expand the macro once per build.
    #[cfg(debug_assertions)]
    let context = tauri::generate_context!("tauri.dev.conf.json");
    #[cfg(not(debug_assertions))]
    let context = tauri::generate_context!();
    context
}

fn main() {
    let mut context = context();
    let probe_window = if probe::enabled() {
        let config = context
            .config_mut()
            .app
            .windows
            .first_mut()
            .expect("main window");
        config.create = false;
        Some(config.clone())
    } else {
        None
    };
    tauri::Builder::default()
        .setup(move |app| {
            if let Some(config) = &probe_window {
                tauri::WebviewWindowBuilder::from_config(app, config)?
                    .initialization_script("window.__CAFE_SPACE_M1_PROBE__ = true;")
                    .build()?;
            }
            Ok(())
        })
        .plugin(external_links())
        .plugin(tauri_plugin_dialog::init())
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
            m0_finish,
            m1_finish
        ])
        .run(context)
        .expect("desktop runtime");
}

#[cfg(test)]
mod tests {
    #[test]
    fn external_links_allow_only_scoped_urls_in_the_local_main_window() {
        assert!(
            tauri::plugin::Plugin::<tauri::Wry>::initialization_script_2(&super::external_links())
                .is_none()
        );
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
        assert_eq!(capability["windows"], serde_json::json!(["main"]));
        assert!(capability.get("remote").is_none());
        let opener: Vec<_> = capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|permission| {
                permission
                    .as_str()
                    .or_else(|| permission["identifier"].as_str())
                    .is_some_and(|id| id.starts_with("opener:"))
            })
            .cloned()
            .collect();
        assert_eq!(
            opener,
            vec![serde_json::json!({
                "identifier": "opener:allow-open-url",
                "allow": [{"url": "http://*"}, {"url": "https://*"}, {"url": "mailto:*"}]
            })]
        );
    }

    #[test]
    fn compiled_url_permission_rejects_other_windows_remote_origins_and_file_commands() {
        let mut context = super::context();
        let authority = context.runtime_authority_mut();
        let remote = tauri::ipc::Origin::Remote {
            url: "https://example.test".parse().unwrap(),
        };
        for command in ["plugin:opener|open_url", "plugin:dialog|open", "m1_finish"] {
            assert!(
                authority
                    .resolve_access(command, "main", "main", &tauri::ipc::Origin::Local)
                    .is_some()
            );
            assert!(
                authority
                    .resolve_access(command, "other", "other", &tauri::ipc::Origin::Local)
                    .is_none()
            );
            assert!(
                authority
                    .resolve_access(command, "main", "main", &remote)
                    .is_none()
            );
        }
        // The folder picker only reads a path; no save, message or confirm dialogs.
        for command in [
            "plugin:opener|open_path",
            "plugin:opener|reveal_item_in_dir",
            "plugin:dialog|save",
            "plugin:dialog|message",
            "plugin:dialog|ask",
            "plugin:dialog|confirm",
        ] {
            assert!(
                authority
                    .resolve_access(command, "main", "main", &tauri::ipc::Origin::Local)
                    .is_none()
            );
        }
    }

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
