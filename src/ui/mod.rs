//! Surface regions of the shell. Each renders a piece of [`AppView`]
//! from its own `impl AppView` block; shared layout constants and the
//! status→color mapping live here.

use gpui::SharedString;
use gpui_kit::component::*;
use gpui_kit::*;

use crate::session::AgentStatus;

pub(crate) mod diff_panel;
pub(crate) mod session_panel;
pub(crate) mod status_bar;
pub(crate) mod terminal;
pub(crate) mod settings_dialog;
pub(crate) mod title_bar;

/// Shared height of the session/changes panel headers (Zed: ~28px).
pub(crate) const PANEL_HEADER_PX: f32 = 32.;
/// Shared height of a selectable single-line list row.
pub(crate) const ROW_PX: f32 = 26.;

/// Unified status→color mapping used by every list row and the status bar.
pub(crate) fn status_dot(status: &AgentStatus, cx: &App) -> Hsla {
    match status {
        AgentStatus::Running => cx.theme().green,
        AgentStatus::Done(_) | AgentStatus::Killed => cx.theme().foreground.opacity(0.4),
        AgentStatus::Error(_) => cx.theme().red,
    }
}

/// Sentence-case 13px medium panel label (Zed panel-header style).
pub(crate) fn panel_label(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .font_medium()
        .text_color(cx.theme().foreground.opacity(0.9))
        .child(text.into())
}

/// Dim count/meta text next to a panel label.
pub(crate) fn meta_text(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().foreground.opacity(0.45))
        .child(text.into())
}

/// Zed reserves accent for activity; list selection is elevated neutral.
pub(crate) fn selection_bg(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.12)
}

/// Neutral hover fill for list rows.
pub(crate) fn hover_bg(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.04)
}
