//! Slim title bar: app name + project breadcrumb.

use gpui_kit::component::TitleBar;
use gpui_kit::component::*;
use gpui_kit::*;

use super::panel_view;
use super::meta_text;
use crate::app::AppView;

/// The breadcrumb's layout box: one definition, read by its own root
/// element and by the title bar's cached mount (`panel_view!` explains
/// why the composer has to state it).
///
/// `h_full` is load-bearing: a cached view's box is laid out as a leaf
/// from this style alone (no content to measure), so a box that leaves
/// its height to its content collapses to zero and the replayed text
/// drops to wherever the parent centers that empty box — the bar's
/// breadcrumb sat against its bottom border.
pub(crate) fn root_style() -> StyleRefinement {
    StyleRefinement::default()
        .flex()
        .items_center()
        .flex_1()
        .h_full()
        .min_w_0()
        .overflow_hidden()
        .px_2()
}

panel_view!(
    /// The "project — session title" text, its own cached view: it
    /// mirrors the visible session's OSC title, which agent CLIs spin,
    /// so a stream frame that changes the title repaints this and the
    /// sidebar row — and nothing else. Rebuilding the whole title bar
    /// (or notifying the app, which fans out to the panels) per spinner
    /// tick is what this avoids.
    PanelView,
    render
);

pub(crate) fn render(
    this: &AppView,
    _window: &mut Window,
    cx: &mut Context<AppView>,
) -> AnyElement {
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

    let mut root = div();
    *root.style() = root_style();
    root.gap_2()
        .text_sm()
        .child(
            meta_text(breadcrumb, cx).min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis(),
        )
        .into_any_element()
}

pub(crate) fn bar(this: &AppView) -> impl IntoElement {
    TitleBar::new()
        // The breadcrumb is a cached view of its own: a title change
        // notifies it directly (see `AppView::subscribe_term`).
        .child(this.breadcrumb.clone().cached(root_style()))
        .child(
            h_flex()
                .items_center()
                .px_2()
                .child(super::settings::button()),
        )
}
