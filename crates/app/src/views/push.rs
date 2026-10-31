use crate::state::AppState;
use boox_core::util::{bytes_to_text, time_ago_text};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex, v_flex,
    label::Label,
    progress::Progress,
    ActiveTheme, Sizable, StyledExt,
};

pub fn render_push_view(state_entity: &Entity<AppState>, cx: &mut App) -> impl IntoElement {
    let (has_snapshot, is_loading, push_queue, active_upload) = {
        let state = state_entity.read(cx);
        let snapshot = state.snapshot.as_ref();
        let queue = snapshot.map(|s| s.push_queue.clone()).unwrap_or_default();
        let up = snapshot.map(|s| s.upload.clone()).filter(|u| u.in_progress || u.last_error.is_some());
        (snapshot.is_some(), state.is_loading, queue, up)
    };

    v_flex()
        .w_full()
        .gap_4()
        // Top Action Bar
        .child(
            h_flex()
                .justify_between()
                .items_center()
                .child(
                    v_flex()
                        .child(Label::new(if has_snapshot {
                            format!("互动文件队列 ({} 项)", push_queue.len())
                        } else {
                            "互动文件队列".to_string()
                        }).font_bold().text_base())
                        .child(Label::new("管理已推送到云端及各 BOOX 设备的书籍与文档").text_xs().text_color(cx.theme().muted_foreground))
                )
                .child({
                    let entity = state_entity.clone();
                    Button::new("push-upload-btn")
                        .primary()
                        .small()
                        .label("选择文件上传并推送")
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |state, cx| {
                                state.pick_and_upload_files(cx);
                            });
                        })
                })
        )
        // Upload Status Card (always visible when in progress or recent error)
        .when_some(active_upload, |this, u| {
            let percent = u.progress_percent.unwrap_or(0.0) as f32;
            let file_name = u.current_file.unwrap_or_else(|| "准备中...".to_string());
            let speed_text = u.speed_bps.map(|s| format!("{}/s", bytes_to_text(s as u64))).unwrap_or_else(|| "-/s".to_string());
            let eta_text = u.eta_seconds.map(|e| format!("剩余 {}s", e as u64)).unwrap_or_else(|| "".to_string());

            this.child(
                div()
                    .rounded_xl()
                    .bg(cx.theme().sidebar)
                    .border_1()
                    .border_color(cx.theme().border)
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Label::new(if u.in_progress { "当前传输:" } else { "最近状态:" }).font_bold().text_sm())
                                    .child(Label::new(file_name).text_sm().text_color(cx.theme().primary))
                            )
                            .child(
                                h_flex()
                                    .gap_3()
                                    .child(Label::new(speed_text).text_xs().font_semibold())
                                    .child(Label::new(eta_text).text_xs().text_color(cx.theme().muted_foreground))
                            )
                    )
                    .when(u.in_progress, |card| {
                        card.child(Progress::new().value(percent))
                    })
                    .when_some(u.last_error, |card, err| {
                        card.child(
                            div()
                                .p_2()
                                .rounded_md()
                                .bg(cx.theme().muted)
                                .text_color(rgb(0xef4444))
                                .text_xs()
                                .child(format!("错误提示: {err}"))
                        )
                    })
            )
        })
        // Push Items Table / List
        .child(
            div()
                .w_full()
                .rounded_xl()
                .bg(cx.theme().sidebar)
                .border_1()
                .border_color(cx.theme().border)
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    if !has_snapshot {
                        div()
                            .p_8()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Label::new(if is_loading {
                                    "正在读取互动文件队列…"
                                } else {
                                    "尚未取得队列数据，请查看刷新提示后重试"
                                })
                                .text_sm()
                                .text_color(cx.theme().muted_foreground),
                            )
                            .into_any_element()
                    } else if push_queue.is_empty() {
                        div()
                            .p_8()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Label::new("队列为空，暂无互动文件").text_sm().text_color(cx.theme().muted_foreground))
                            .into_any_element()
                    } else {
                        v_flex()
                            .gap_2()
                            .children(push_queue.iter().enumerate().map(|(idx, item)| {
                                let doc_id = item.id.clone();
                                let title = item.name.clone();
                                let time_text = item.updated_at.map(|t| time_ago_text(t as u128)).unwrap_or_else(|| "-".to_string());
                                let size_text = item.size.map(bytes_to_text).unwrap_or_default();
                                let format_text = item.format.clone().unwrap_or_else(|| "PDF".to_string()).to_ascii_uppercase();

                                h_flex()
                                    .w_full()
                                    .p_3()
                                    .rounded_lg()
                                    .bg(cx.theme().background)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        h_flex()
                                            .gap_3()
                                            .items_center()
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_md()
                                                    .bg(cx.theme().primary.opacity(0.15))
                                                    .text_color(cx.theme().primary)
                                                    .font_bold()
                                                    .text_xs()
                                                    .child(format_text)
                                            )
                                            .child(
                                                v_flex()
                                                    .child(Label::new(title).font_semibold().text_sm())
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .child(Label::new(size_text).text_xs().text_color(cx.theme().muted_foreground))
                                                            .child(Label::new("·").text_xs().text_color(cx.theme().muted_foreground))
                                                            .child(Label::new(time_text).text_xs().text_color(cx.theme().muted_foreground))
                                                    )
                                            )
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child({
                                                let entity = state_entity.clone();
                                                let id = doc_id.clone();
                                                Button::new(SharedString::from(format!("queue-resend-{}", idx)))
                                                    .outline()
                                                    .small()
                                                    .label("重推到设备")
                                                    .on_click(move |_, _, cx| {
                                                        entity.update(cx, |state, cx| {
                                                            state.resend_push_item(id.clone(), cx);
                                                        });
                                                    })
                                            })
                                            .child({
                                                let entity = state_entity.clone();
                                                let id = doc_id.clone();
                                                Button::new(SharedString::from(format!("queue-delete-{}", idx)))
                                                    .ghost()
                                                    .small()
                                                    .label("删除")
                                                    .on_click(move |_, _, cx| {
                                                        entity.update(cx, |state, cx| {
                                                            state.delete_push_item(id.clone(), cx);
                                                        });
                                                    })
                                            })
                                    )
                            }))
                            .into_any_element()
                    }
                )
        )
}
