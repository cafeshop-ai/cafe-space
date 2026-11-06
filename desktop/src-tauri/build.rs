fn main() {
    embed_daemon();
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "daemon_open",
            "daemon_send",
            "daemon_ack",
            "daemon_close",
            "daemon_installation_info",
            "daemon_install",
            "daemon_start",
            "daemon_update",
            "daemon_cancel_update",
            "daemon_recover",
            "m0_finish",
            "m1_finish",
        ]),
    ))
    .expect("desktop build configuration");
}

fn embed_daemon() {
    use std::{env, path::PathBuf};
    let profile = env::var("PROFILE").expect("Cargo profile");
    let flavor = match profile.as_str() {
        "debug" => cafe_space_ipc::BuildFlavor::Dev,
        "release" => cafe_space_ipc::BuildFlavor::Release,
        _ => panic!("build the matching daemon with the standard debug or release profile"),
    };
    let suffix = if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        ".exe"
    } else {
        ""
    };
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let metadata = std::process::Command::new(env::var_os("CARGO").unwrap())
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .expect("read daemon Cargo metadata without executing it");
    assert!(
        metadata.status.success(),
        "daemon Cargo metadata must resolve offline"
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&metadata.stdout).expect("Cargo metadata JSON");
    let package = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "cafe-spaced")
        .expect("daemon package");
    let source = PathBuf::from(metadata["target_directory"].as_str().unwrap())
        .join(profile)
        .join(format!("{}{suffix}", flavor.daemon_name()));
    println!(
        "cargo:rustc-env=CAFE_BUNDLED_DAEMON_VERSION={}",
        package["version"].as_str().unwrap()
    );
    println!("cargo:rerun-if-changed={}", source.display());
    let snapshot = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("cafe-spaced.bundle");
    // Snapshot the artifact: rustc must embed the exact bytes whose digest is compiled alongside it.
    std::fs::copy(&source, &snapshot)
        .expect("build the matching cafe-spaced binary before the desktop shell");
    let hash =
        cafe_space_ipc::installation::binary_hash(&snapshot).expect("bounded daemon artifact");
    println!("cargo:rustc-env=CAFE_BUNDLED_DAEMON={}", snapshot.display());
    println!("cargo:rustc-env=CAFE_BUNDLED_DAEMON_HASH={hash}");
    println!(
        "cargo:rustc-env=CAFE_BUNDLED_DAEMON_NAME={}",
        flavor.daemon_name()
    );
}
