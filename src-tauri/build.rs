fn main() {
    // 设置 macOS 最低部署目标：High Sierra 10.13（最后支持多款旧款 Intel Mac）
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=10.13");

    // Windows: tauri-build embeds its app manifest (Common Controls v6) into the
    // app binary only. The `cargo test` executable links tauri too, which
    // imports TaskDialogIndirect from Common Controls v6, and without the
    // manifest it fails to start (STATUS_ENTRYPOINT_NOT_FOUND). So embed the
    // same manifest ourselves, for every target, and tell tauri-build not to.
    let mut attributes = tauri_build::Attributes::new();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    }

    tauri_build::try_build(attributes).expect("failed to run tauri-build")
}
