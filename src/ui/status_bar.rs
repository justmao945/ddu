//! Bottom status bar: panel toggles plus live context — the current
//! session (title · status) on the left, the project's branch and
//! working-tree change stats on the right.

use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::*;

use super::status_dot;
use crate::app::AppView;

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    StatusBar::new()
        .left(
            h_flex()
                .items_center()
                .gap_2()
                .px_1()
                .child(
                    Button::new("toggle-sessions")
                        .icon(if this.show_sessions {
                            IconName::PanelLeftClose
                        } else {
                            IconName::PanelLeftOpen
                        })
                        .ghost()
                        .small()
                        .tooltip("Toggle sidebar (⌘\\)")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_sessions = !this.show_sessions;
                            cx.notify();
                        })),
                )
                .children(this.current_session().map(|s| {
                    let dot = status_dot(&s.status, cx);
                    let label = s.status.label().to_string();
                    let title = s.title.clone();
                    let kind = s.kind_label(cx);
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .min_w_0()
                        .child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(dot))
                        .child(
                            div()
                                .min_w_0()
                                .text_xs()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(cx.theme().foreground.opacity(0.75))
                                .child(title),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(cx.theme().foreground.opacity(0.45))
                                .child(format!("{kind} · {label}")),
                        )
                })),
        )
        .right(
            h_flex()
                .items_center()
                .gap_2()
                .px_1()
                .children(diff_summary(this, cx))
                .child(
                    Button::new("toggle-diff")
                        .icon(if this.show_diff {
                            IconName::PanelRightClose
                        } else {
                            IconName::PanelRightOpen
                        })
                        .ghost()
                        .small()
                        .tooltip("Toggle changes (⌘B)")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_diff = !this.show_diff;
                            cx.notify();
                        })),
                ),
        )
}

/// `master · 12 files · +2642 −440` for the current project, if any.
fn diff_summary(
    this: &AppView,
    cx: &mut Context<AppView>,
) -> Option<impl IntoElement + use<>> {
    let diff = this.diff.as_ref()?;
    if diff.is_empty() {
        return None;
    }
    let added: usize = diff.files.iter().map(|f| f.added).sum();
    let removed: usize = diff.files.iter().map(|f| f.removed).sum();
    let branch = diff.branch.clone().unwrap_or_else(|| "HEAD".into());
    let mono = cx.theme().mono_font_family.clone();

    Some(
        h_flex()
            .items_center()
            .gap_2()
            .text_xs()
            .child(
                div()
                    .text_color(cx.theme().foreground.opacity(0.55))
                    .child(branch),
            )
            .child(
                div()
                    .font_family(mono)
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .text_color(cx.theme().green)
                            .child(format!("+{added}")),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().red)
                            .child(format!("−{removed}")),
                    ),
            )
            .child(
                div()
                    .text_color(cx.theme().foreground.opacity(0.45))
                    .child(format!(
                        "{} file{}",
                        diff.files.len(),
                        if diff.files.len() == 1 { "" } else { "s" }
                    )),
            ),
    )
}
