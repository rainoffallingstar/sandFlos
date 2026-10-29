use auto_launch::{AutoLaunch, AutoLaunchBuilder};
use std::fs;

const AUTOSTART_MARKER: &str = "autostart_initialized";
const APP_NAME: &str = "sandFlos";

pub fn build_auto_launch() -> Result<AutoLaunch, String> {
    if !AutoLaunch::is_support() {
        return Err("当前平台不支持开机自启动".to_string());
    }
    let exe_path = std::env::current_exe().map_err(|err| err.to_string())?;
    let exe = exe_path.to_string_lossy().to_string();
    let mut builder = AutoLaunchBuilder::new();
    builder.set_app_name(APP_NAME).set_app_path(&exe);
    #[cfg(target_os = "macos")]
    builder.set_use_launch_agent(true);
    builder.build().map_err(|err| err.to_string())
}

pub fn is_auto_launch_enabled() -> bool {
    match build_auto_launch() {
        Ok(auto) => auto.is_enabled().unwrap_or(false),
        Err(_) => false,
    }
}

pub fn toggle_auto_launch() -> Result<bool, String> {
    let auto = build_auto_launch()?;
    let enabled = auto.is_enabled().unwrap_or(false);
    if enabled {
        auto.disable().map_err(|err| err.to_string())?;
        Ok(false)
    } else {
        auto.enable().map_err(|err| err.to_string())?;
        Ok(true)
    }
}

pub fn initialize_auto_launch_default() {
    let dir = boox_core::state::app_data_dir();
    let marker_path = dir.join(AUTOSTART_MARKER);
    if !marker_path.exists() {
        if let Ok(auto) = build_auto_launch() {
            let _ = auto.enable();
        }
        let _ = fs::write(marker_path, b"initialized");
    }
}
