fn main() {
    // Windows' resource compiler does not correctly handle the apostrophe in
    // this workspace's absolute path. Stage only the build icon in the system
    // temp directory so the generated resource file receives a safe path.
    let source_icon = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("icons/icon.ico");
    let staged_icon = std::env::temp_dir().join("gif-library-build-icon.ico");
    std::fs::copy(&source_icon, &staged_icon).expect("failed to stage Windows build icon");

    let windows = tauri_build::WindowsAttributes::new().window_icon_path(staged_icon);
    let attributes = tauri_build::Attributes::new().windows_attributes(windows);

    tauri_build::try_build(attributes).expect("failed to run Tauri build script");
}
