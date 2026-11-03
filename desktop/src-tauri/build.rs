fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "daemon_open",
            "daemon_send",
            "daemon_ack",
            "daemon_close",
            "m0_finish",
        ]),
    ))
    .expect("desktop build configuration");
}
