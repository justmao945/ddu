//! Session list panel (Zed threads-panel style): header with count and
//! new-session action, two-line thread items - title over dim agent cmd -
//! with neutral rounded selection (accent is reserved for activity).

use gpui_kit::component::*;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;

use super::{
    PANEL_HEADER_PX, hover_bg, meta_text, panel_label, selection_bg, status_dot,
};
use crate::app::AppView;

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let count = this.current_project().sessions.len();

    v_flex()
        .h_full()
        .w_full()
        .bg(cx.theme().sidebar)
        .child(
            div()
                .h(px(PANEL_HEADER_PX))
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .child(panel_label("Sessions", cx))
                .child(meta_text(format!("{count}"), cx))
                .child(div().flex_1())
                .child(
                    Button::new("new-session")
                        .icon(IconName::Plus)
                        .ghost()
                        .small()
                        .tooltip("New session (⌘T)")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.add_session(cx);
                        })),
                ),
        )
        .child(rows(this, cx))
}

fn rows(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let project = this.current_project();
    let radius = cx.theme().radius;
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);
    let fg = cx.theme().foreground;
    v_flex()
        .id("session-rows")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p_2()
        .gap_1()
        .children(
        project.sessions.iter().enumerate().map(|(six, s)| {
            let active = six == this.current_session;
            let dot = status_dot(&s.status, cx);

            div()
                .id(("session-row", six))
                .flex()
                .items_start()
                .gap_2()
                .px_2()
                .py_1()
                .rounded(radius)
                .cursor_pointer()
                .map(|el| if active { el.bg(active_bg) } else { el })
                .hover(move |el| if active { el } else { el.bg(hov_bg) })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let p = this.current_project;
                    this.select_session(p, six, cx);
                }))
                .child(div().mt(px(5.)).size(px(7.)).flex_shrink_0().rounded_full().bg(dot))
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(if active { fg } else { fg.opacity(0.85) })
                                .child(s.title.clone()),
                        )
                        .child(meta_text(s.cmd.clone(), cx)),
                )
        }),
    )
}
