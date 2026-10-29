use std::cell::RefCell;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Login,
    Upload,
    Refresh,
    ToggleAutostart,
    Quit,
}

pub struct SystemTray {
    pub _tray: TrayIcon,
    pub login_id: MenuId,
    pub upload_id: MenuId,
    pub refresh_id: MenuId,
    pub reading_stats_id: MenuId,
    pub autostart_id: MenuId,
    pub quit_id: MenuId,
    pub reading_stats_item: MenuItem,
    pub autostart_item: MenuItem,
}

thread_local! {
    static TRAY_INSTANCE: RefCell<Option<SystemTray>> = const { RefCell::new(None) };
}

pub fn setup_tray() -> Result<(), String> {
    let menu = Menu::new();

    let login_item = MenuItem::new("登录并授权", true, None);
    let upload_item = MenuItem::new("上传文件", true, None);
    let refresh_item = MenuItem::new("刷新仪表盘", true, None);
    let reading_stats_item = MenuItem::new("阅读统计指标: 未授权", false, None);

    let autostart_enabled = crate::autostart::is_auto_launch_enabled();
    let autostart_text = if autostart_enabled {
        "开机自启动: 开"
    } else {
        "开机自启动: 关"
    };
    let autostart_item = MenuItem::new(autostart_text, true, None);

    let quit_item = MenuItem::new("退出", true, None);

    let login_id = login_item.id().clone();
    let upload_id = upload_item.id().clone();
    let refresh_id = refresh_item.id().clone();
    let reading_stats_id = reading_stats_item.id().clone();
    let autostart_id = autostart_item.id().clone();
    let quit_id = quit_item.id().clone();

    menu.append(&login_item).map_err(|e| e.to_string())?;
    menu.append(&upload_item).map_err(|e| e.to_string())?;
    menu.append(&refresh_item).map_err(|e| e.to_string())?;
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
    menu.append(&reading_stats_item).map_err(|e| e.to_string())?;
    menu.append(&autostart_item).map_err(|e| e.to_string())?;
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
    menu.append(&quit_item).map_err(|e| e.to_string())?;

    let icon = create_default_tray_icon();

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("sandFlos 控制中心")
        .with_icon(icon)
        .build()
        .map_err(|e| e.to_string())?;

    TRAY_INSTANCE.with(|cell| {
        *cell.borrow_mut() = Some(SystemTray {
            _tray: tray,
            login_id,
            upload_id,
            refresh_id,
            reading_stats_id,
            autostart_id,
            quit_id,
            reading_stats_item,
            autostart_item,
        });
    });

    Ok(())
}

fn create_default_tray_icon() -> tray_icon::Icon {
    let width = 32;
    let height = 32;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let dx = (x as i32 - 16).abs();
            let dy = (y as i32 - 16).abs();
            if dx <= 10 && dy <= 10 {
                rgba.extend_from_slice(&[10, 132, 255, 255]); // blue icon
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, width, height).unwrap()
}

pub fn update_reading_stats_label(text: &str) {
    TRAY_INSTANCE.with(|cell| {
        if let Some(tray) = cell.borrow().as_ref() {
            let truncated = boox_core::util::truncate_menu_title(text);
            let _ = tray.reading_stats_item.set_text(truncated);
        }
    });
}

pub fn update_autostart_label(enabled: bool) {
    TRAY_INSTANCE.with(|cell| {
        if let Some(tray) = cell.borrow().as_ref() {
            let text = if enabled {
                "开机自启动: 开"
            } else {
                "开机自启动: 关"
            };
            let _ = tray.autostart_item.set_text(text);
        }
    });
}

pub fn classify_menu_event(event: &MenuEvent) -> Option<TrayAction> {
    let target_id = event.id();
    TRAY_INSTANCE.with(|cell| {
        if let Some(tray) = cell.borrow().as_ref() {
            if target_id == &tray.login_id {
                Some(TrayAction::Login)
            } else if target_id == &tray.upload_id {
                Some(TrayAction::Upload)
            } else if target_id == &tray.refresh_id {
                Some(TrayAction::Refresh)
            } else if target_id == &tray.autostart_id {
                Some(TrayAction::ToggleAutostart)
            } else if target_id == &tray.quit_id {
                Some(TrayAction::Quit)
            } else {
                None
            }
        } else {
            None
        }
    })
}
