fn main() {
    // The tray app (and its embedded UI) exists on Windows only.
    #[cfg(windows)]
    tauri_build::build();
}
