fn main() {
    // Zig's Intel Mach-O linker may leave no room for LC_CODE_SIGNATURE.
    // Reserve header space so the cross-built executable can be signed.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-headerpad,0x1000");
    }
    // Every command gets its own permission (allow-<name>) so the capability files can
    // give YouTube pages only the two commands they need.
    let commands = &[
        "app_boot", "tabs_state", "tab_new", "tab_activate", "tab_close", "tab_go", "tab_back", "tab_forward", "tab_reload", "tab_open_video",
        "tab_hibernate", "layout", "native_visible", "open_external", "trust_status", "trust_install", "signin_popup", "page_open_video",
        "page_state", "e2e_result", "channel_unsubscribe", "export_diagnostics",
    ];
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(commands))).expect("tauri build");
}
