fn main() {
    // 设置 macOS 最低部署目标：High Sierra 10.13（最后支持多款旧款 Intel Mac）
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=10.13");

    tauri_build::build()
}
