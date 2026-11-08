fn main() {
    // Only these commands are callable from the window; capabilities/main.json allows exactly them.
    let commands = &[
        "get_snapshot",
        "refresh_now",
        "detect_sources",
        "get_settings",
        "update_settings",
        "connect",
        "add_account",
        "open_cli",
        "remove_account",
        "set_hidden",
        "set_pinned",
        "set_label",
        "connect_source",
        "open_url",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("failed to run tauri-build");
}
