mod autostart;
mod components;
mod state;
mod tray;
mod views;

use gpui::*;
use gpui_component::{
    h_flex, v_flex,
    scroll::ScrollableElement,
    ActiveTheme, Root,
};
use state::{AppState, ViewTab};
use tray_icon::menu::MenuEvent;

struct MainView {
    state: Entity<AppState>,
}

impl MainView {
    pub fn new(state: Entity<AppState>) -> Self {
        Self { state }
    }
}

impl Render for MainView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_tab = self.state.read(cx).active_tab;

        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(components::sidebar::render_sidebar(&self.state, cx))
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .child(components::toolbar::render_toolbar(&self.state, cx))
                    .child(
                        v_flex()
                            .id("main-scroll-area")
                            .flex_1()
                            .h_full()
                            .overflow_y_scrollbar()
                            .p_5()
                            .child(match active_tab {
                                ViewTab::Overview => {
                                    views::overview::render_overview_view(&self.state, cx)
                                        .into_any_element()
                                }
                                ViewTab::Push => {
                                    views::push::render_push_view(&self.state, cx)
                                        .into_any_element()
                                }
                                ViewTab::Devices => {
                                    views::devices::render_devices_view(&self.state, cx)
                                        .into_any_element()
                                }
                                ViewTab::Reading => {
                                    views::reading::render_reading_view(&self.state, cx)
                                        .into_any_element()
                                }
                                ViewTab::Zotero => {
                                    views::zotero::render_zotero_view(&self.state, cx)
                                        .into_any_element()
                                }
                                ViewTab::Calibre => {
                                    views::calibre::render_calibre_view(&self.state, cx)
                                        .into_any_element()
                                }
                            })
                    )
            )
    }
}

fn main() {
    let app = Application::new();
    app.run(move |cx| {
        gpui_component::init(cx);

        // Initialize system tray
        let _ = tray::setup_tray();

        // Create global app state entity
        let state_entity = cx.new(|_cx| AppState::new());

        // Start auto-refresh and initial data load
        state_entity.update(cx, |state, cx| {
            state.start_auto_refresh_loop(cx);
        });

        // Listen for Tray Menu events
        let state_for_tray = state_entity.clone();
        cx.spawn(async move |cx| {
            let menu_channel = MenuEvent::receiver();
            loop {
                if let Ok(event) = menu_channel.try_recv() {
                    if let Some(action) = tray::classify_menu_event(&event) {
                        match action {
                            tray::TrayAction::Login => {
                                let _ = state_for_tray.update(cx, |state, cx| {
                                    state.start_login(cx);
                                });
                            }
                            tray::TrayAction::Upload => {
                                let _ = state_for_tray.update(cx, |state, cx| {
                                    state.pick_and_upload_files(cx);
                                });
                            }
                            tray::TrayAction::Refresh => {
                                let _ = state_for_tray.update(cx, |state, cx| {
                                    state.refresh_snapshot(cx);
                                });
                            }
                            tray::TrayAction::ToggleAutostart => {
                                if let Ok(new_status) = crate::autostart::toggle_auto_launch() {
                                    tray::update_autostart_label(new_status);
                                }
                            }
                            tray::TrayAction::Quit => {
                                std::process::exit(0);
                            }
                        }
                    }
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1120.0), px(820.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("sandFlos 控制中心".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            ..Default::default()
        };

        let state_for_window = state_entity.clone();
        let _ = cx.open_window(options, |window, cx| {
            let view = cx.new(|_cx| MainView::new(state_for_window));
            cx.new(|cx| Root::new(view, window, cx))
        });
    });
}
