//! Slim title bar: app name + project breadcrumb.

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use gpui_kit::component::*;

use super::meta_text;
use crate::app::AppView;

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let project = this.current_project();
    let breadcrumb = match this.current_session() {
        Some(s) => format!("{} — {}", project.name, s.title),
        None => project.name.clone(),
    };

    TitleBar::new()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .text_sm()
                .child(div().font_medium().child("Day Day Up"))
                .child(meta_text(breadcrumb, cx)),
        )
        .child(
            h_flex()
                .items_center()
                .px_2()
                .child(super::settings_dialog::button()),
        )
}
