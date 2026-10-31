use boox_core::dashboard::DashboardRefreshInput;
use boox_core::models::{
    CalibreBookSummary, CalibreConnectionState, DashboardSnapshot,
    ZoteroConnectionState, ZoteroItemSummary,
};
use boox_core::util::{today_ymd, unix_ms_now};
use gpui::*;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewTab {
    #[default]
    Overview,
    Push,
    Devices,
    Reading,
    Zotero,
    Calibre,
}

impl ViewTab {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Overview => "概览",
            Self::Push => "互动文件",
            Self::Devices => "设备与互传",
            Self::Reading => "阅读指标",
            Self::Zotero => "Zotero 附件工作流",
            Self::Calibre => "Calibre 书库工作流",
        }
    }

    pub fn kicker(&self) -> &'static str {
        match self {
            Self::Overview => "概览",
            Self::Push => "互动文件",
            Self::Devices => "设备",
            Self::Reading => "阅读",
            Self::Zotero => "Zotero",
            Self::Calibre => "Calibre",
        }
    }

    pub fn subtitle(&self) -> &'static str {
        match self {
            Self::Overview => "统一查看授权状态、上传进度、设备摘要与阅读摘要。",
            Self::Push => "管理互动文件、查看上传状态，并直接重推到设备。",
            Self::Devices => "查看在线设备、同局域网设备，并打开互传地址。",
            Self::Reading => "聚合今日阅读、本周时长和累计阅读完成情况。",
            Self::Zotero => "查看最近文献附件、按需推送，并在缺失本地文件时走 WebDAV 拉取链路。",
            Self::Calibre => "直接读取 metadata.db，展示最近书籍并按数据库标题推送到 BOOX。",
        }
    }

    pub fn badge(&self) -> &'static str {
        match self {
            Self::Overview => "Workspace",
            Self::Push => "Queue",
            Self::Devices => "Devices",
            Self::Reading => "Reading",
            Self::Zotero => "Zotero",
            Self::Calibre => "Calibre",
        }
    }
}

pub struct ZoteroUiState {
    pub connection: Option<ZoteroConnectionState>,
    pub items: Vec<ZoteroItemSummary>,
    pub is_loading: bool,
    pub search_query: String,
    pub pushing_id: Option<i64>,
}

impl Default for ZoteroUiState {
    fn default() -> Self {
        Self {
            connection: None,
            items: Vec::new(),
            is_loading: false,
            search_query: String::new(),
            pushing_id: None,
        }
    }
}

pub struct CalibreUiState {
    pub connection: Option<CalibreConnectionState>,
    pub books: Vec<CalibreBookSummary>,
    pub is_loading: bool,
    pub search_query: String,
    pub pushing_id: Option<i64>,
}

impl Default for CalibreUiState {
    fn default() -> Self {
        Self {
            connection: None,
            books: Vec::new(),
            is_loading: false,
            search_query: String::new(),
            pushing_id: None,
        }
    }
}

pub struct AppState {
    pub snapshot: Option<DashboardSnapshot>,
    pub active_tab: ViewTab,
    pub is_loading: bool,
    pub refresh_interval_minutes: f64,
    pub last_sync_time_ms: u128,
    pub zotero: ZoteroUiState,
    pub calibre: CalibreUiState,
    pub status_notification: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        boox_core::state::hydrate_auth_state();
        boox_core::diagnostics::init();
        crate::autostart::initialize_auto_launch_default();

        Self {
            snapshot: None,
            active_tab: ViewTab::Overview,
            is_loading: false,
            refresh_interval_minutes: 1.0,
            last_sync_time_ms: unix_ms_now(),
            zotero: ZoteroUiState::default(),
            calibre: CalibreUiState::default(),
            status_notification: None,
        }
    }

    pub fn set_active_tab(&mut self, tab: ViewTab, cx: &mut Context<Self>) {
        self.active_tab = tab;
        match tab {
            ViewTab::Zotero => self.load_zotero(cx),
            ViewTab::Calibre => self.load_calibre(cx),
            _ => {}
        }
        cx.notify();
    }

    pub fn set_refresh_interval(&mut self, minutes: f64, cx: &mut Context<Self>) {
        self.refresh_interval_minutes = minutes.max(0.1);
        cx.notify();
    }

    pub fn refresh_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.is_loading {
            return;
        }
        self.is_loading = true;
        cx.notify();

        // The credential and upload projection still come from the legacy
        // core globals during this migration slice. The dashboard result itself
        // is explicit: the app owns the snapshot and no core dashboard cache is
        // written.
        let input = DashboardRefreshInput::new(
            boox_core::state::get_auth_state().token.unwrap_or_default(),
            boox_core::dashboard::current_upload_snapshot(),
            today_ymd(),
        );

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { boox_core::dashboard::refresh_dashboard_snapshot(&input) })
                .await;

            let _ = this.update(cx, |state, cx| {
                state.is_loading = false;
                match result {
                    Ok(next) => {
                        let label = boox_core::dashboard::build_reading_metrics_label(&next);
                        state.snapshot = Some(next);
                        state.last_sync_time_ms = unix_ms_now();
                        state.status_notification = Some("仪表盘已刷新".to_string());
                        crate::tray::update_reading_stats_label(&label);
                    }
                    Err(error) => {
                        state.status_notification = Some(format!("刷新失败: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn start_auto_refresh_loop(&mut self, cx: &mut Context<Self>) {
        self.refresh_snapshot(cx);

        cx.spawn(async move |this, cx| {
            loop {
                let interval_secs = this
                    .read_with(cx, |state, _cx| state.refresh_interval_minutes * 60.0)
                    .unwrap_or(60.0);

                cx.background_executor()
                    .timer(Duration::from_secs_f64(interval_secs.max(5.0)))
                    .await;

                let res = this.update(cx, |state, cx| {
                    state.refresh_snapshot(cx);
                });
                if res.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn start_login(&mut self, cx: &mut Context<Self>) {
        let res = boox_core::auth::start_login_flow(|url| {
            open::that(url).map_err(|err| err.to_string())
        });
        if let Err(err) = res {
            self.status_notification = Some(format!("启动登录失败: {err}"));
        } else {
            self.status_notification = Some("已在默认浏览器中打开登录页面".to_string());
        }
        cx.notify();
    }

    pub fn pick_and_upload_files(&mut self, cx: &mut Context<Self>) {
        if !boox_core::state::try_begin_upload_task() {
            self.status_notification = Some("已有上传任务在执行中".to_string());
            cx.notify();
            return;
        }

        cx.spawn(async move |this, cx| {
            let files_opt = cx
                .background_executor()
                .spawn(async move {
                    rfd::FileDialog::new()
                        .set_title("选择待上传并推送到 BOOX 的文件")
                        .pick_files()
                })
                .await;

            if let Some(files) = files_opt {
                if files.is_empty() {
                    boox_core::state::finish_upload_task();
                    return;
                }
                let count = files.len();
                let _ = this.update(cx, |state, cx| {
                    state.status_notification = Some(format!("开始上传 {} 个文件...", count));
                    cx.notify();
                });

                let upload_result = cx
                    .background_executor()
                    .spawn(async move {
                        boox_core::push::upload_files_blocking_with_active_task(files)
                    })
                    .await;

                let _ = this.update(cx, |state, cx| {
                    match upload_result {
                        Ok(snapshot) => {
                            state.snapshot = Some(snapshot);
                            state.status_notification = Some("上传成功并已推送到设备！".to_string());
                        }
                        Err(err) => {
                            state.status_notification = Some(format!("上传失败: {err}"));
                        }
                    }
                    cx.notify();
                });
            } else {
                boox_core::state::finish_upload_task();
            }
        })
        .detach();
    }

    pub fn resend_push_item(&mut self, id: String, cx: &mut Context<Self>) {
        let id_for_task = id.clone();
        self.status_notification = Some("正在重新推送...".to_string());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { boox_core::push::dashboard_push_resend(id_for_task) })
                .await;

            let _ = this.update(cx, |state, cx| {
                match res {
                    Ok(snapshot) => {
                        state.snapshot = Some(snapshot);
                        state.status_notification = Some("已成功重新推送！".to_string());
                    }
                    Err(err) => {
                        state.status_notification = Some(format!("重新推送失败: {err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn delete_push_item(&mut self, id: String, cx: &mut Context<Self>) {
        let id_for_task = id.clone();
        self.status_notification = Some("正在删除记录...".to_string());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { boox_core::push::dashboard_push_delete(id_for_task) })
                .await;

            let _ = this.update(cx, |state, cx| {
                match res {
                    Ok(snapshot) => {
                        state.snapshot = Some(snapshot);
                        state.status_notification = Some("已成功删除记录！".to_string());
                    }
                    Err(err) => {
                        state.status_notification = Some(format!("删除失败: {err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn open_transfer_url(&mut self, host: String, cx: &mut Context<Self>) {
        let res = boox_core::push::dashboard_open_transfer_host(host, |url| {
            open::that(url).map_err(|err| err.to_string())
        });
        if let Err(err) = res {
            self.status_notification = Some(format!("打开设备互传失败: {err}"));
            cx.notify();
        }
    }

    pub fn load_zotero(&mut self, cx: &mut Context<Self>) {
        self.zotero.is_loading = true;
        cx.notify();

        let search = if self.zotero.search_query.is_empty() {
            None
        } else {
            Some(self.zotero.search_query.clone())
        };

        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async move { boox_core::zotero::zotero_status() })
                .await;
            let items = cx
                .background_executor()
                .spawn(async move { boox_core::zotero::list_recent_items_inner(Some(50), None, search) })
                .await;

            let _ = this.update(cx, |state, cx| {
                state.zotero.is_loading = false;
                state.zotero.connection = status.ok();
                state.zotero.items = items.unwrap_or_default();
                cx.notify();
            });
        })
        .detach();
    }

    pub fn push_zotero_item(&mut self, attachment_id: i64, cx: &mut Context<Self>) {
        self.zotero.pushing_id = Some(attachment_id);
        self.status_notification = Some("正在从 Zotero 准备附件并上传...".to_string());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { boox_core::zotero::zotero_push_attachment(attachment_id) })
                .await;

            let _ = this.update(cx, |state, cx| {
                state.zotero.pushing_id = None;
                match res {
                    Ok(snapshot) => {
                        state.snapshot = Some(snapshot);
                        state.status_notification = Some("Zotero 附件推送成功！".to_string());
                    }
                    Err(err) => {
                        state.status_notification = Some(format!("Zotero 附件推送失败: {err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_calibre(&mut self, cx: &mut Context<Self>) {
        self.calibre.is_loading = true;
        cx.notify();

        let search = if self.calibre.search_query.is_empty() {
            None
        } else {
            Some(self.calibre.search_query.clone())
        };

        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async move { boox_core::calibre::calibre_status() })
                .await;
            let books = cx
                .background_executor()
                .spawn(async move { boox_core::calibre::list_recent_books_inner(Some(50), None, search) })
                .await;

            let _ = this.update(cx, |state, cx| {
                state.calibre.is_loading = false;
                state.calibre.connection = status.ok();
                state.calibre.books = books.unwrap_or_default();
                cx.notify();
            });
        })
        .detach();
    }

    pub fn push_calibre_book_format(&mut self, library_dir: String, data_id: i64, cx: &mut Context<Self>) {
        self.calibre.pushing_id = Some(data_id);
        self.status_notification = Some("正在从 Calibre 准备书籍格式并上传...".to_string());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { boox_core::calibre::calibre_push_format(library_dir, data_id) })
                .await;

            let _ = this.update(cx, |state, cx| {
                state.calibre.pushing_id = None;
                match res {
                    Ok(snapshot) => {
                        state.snapshot = Some(snapshot);
                        state.status_notification = Some("Calibre 书籍推送成功！".to_string());
                    }
                    Err(err) => {
                        state.status_notification = Some(format!("Calibre 书籍推送失败: {err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
