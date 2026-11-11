fn main() {
    // Only these commands are callable from the window; capabilities/main.json allows exactly them.
    // `commands::tests::acl_lists_every_command` checks this list against main.rs and main.json.
    let commands = &[
        "get_snapshot",
        "refresh_now",
        "detect",
        "get_settings",
        "update_settings",
        "connect",
        "add_account",
        "open_cli",
        "remove_account",
        "set_hidden",
        "set_label",
        "connect_source",
        "open_url",
        "set_zoom",
        "set_pill_scale",
        "set_pill_slots",
        "set_pill_locked",
        "flyout_hover",
        "flyout_pointer",
        "flyout_open_popover",
        "flyout_close",
        "flyout_size",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(commands)),
    )
    .expect("failed to run tauri-build");
}
