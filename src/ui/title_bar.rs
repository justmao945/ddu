//! Slim title bar: app name + project breadcrumb.

use gpui_kit::component::TitleBar;
use gpui_kit::component::*;
use gpui_kit::*;

use super::meta_text;
use crate::app::AppView;

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let project = this.current_project();
    let session_title = this.current_session().and_then(|s| {
        s.term
            .as_ref()
            .map(|t| t.read(cx).title())
            .unwrap_or_default()
            .or_else(|| Some(s.title.clone()))
    });
    let breadcrumb = match (project, session_title) {
        (Some(p), Some(title)) => format!("{} — {}", p.name, title),
        (Some(p), None) => p.name.clone(),
        // Empty workspace: bare app name.
        (None, _) => "ddu".to_string(),
    };

    TitleBar::new()
        .child(
            div()
                .flex()
                .items_center()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .gap_2()
                .px_2()
                .text_sm()
                .child(
                    meta_text(breadcrumb, cx)
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis(),
                ),
        )
        .child(
            h_flex()
                .items_center()
                .px_2()
                .child(super::settings::button()),
        )
}
