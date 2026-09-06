//! Surface regions of the shell. Each renders a piece of [`AppView`]
//! from its own `impl AppView` block; shared layout constants and the
//! status→color mapping live here.

use gpui::SharedString;
use gpui_kit::component::*;
use gpui_kit::*;

pub(crate) mod diff_panel;
pub(crate) mod session_panel;
pub(crate) mod settings_window;
pub(crate) mod status_bar;
pub(crate) mod terminal;
pub(crate) mod title_bar;

/// Shared height of the session/changes panel headers (Zed: ~28px).
pub(crate) const PANEL_HEADER_PX: f32 = 32.;
/// Shared height of a selectable single-line list row.
pub(crate) const ROW_PX: f32 = 26.;

/// Dim count/meta text next to a panel label.
pub(crate) fn meta_text(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().foreground.opacity(0.45))
        .child(text.into())
}

/// Custom icons shipped from `assets/icons` and registered in
/// `main.rs::AppAssets`, drop-in usable wherever an `IconName` fits
/// (via the `IconNamed` trait).
#[derive(Clone, Copy)]
pub(crate) enum AppIcon {
    FolderPlus,
}

impl gpui_kit::component::IconNamed for AppIcon {
    fn path(self) -> SharedString {
        match self {
            AppIcon::FolderPlus => "icons/folder-plus.svg".into(),
        }
    }
}

/// Brand/status icon for a session kind (`terminal`, `claude`, `codex`,
/// `omp`, custom). Shared by the sidebar menus and the settings window.
pub(crate) fn agent_icon(kind: &str) -> Icon {
    match kind {
        "terminal" => Icon::new(IconName::SquareTerminal),
        "claude" => Icon::default().path("icons/claude.svg"),
        "codex" => Icon::default().path("icons/openai.svg"),
        "omp" => Icon::default().path("icons/omp.svg"),
        _ => Icon::new(IconName::Bot),
    }
}

/// Zed reserves accent for activity; list selection is elevated neutral.
pub(crate) fn selection_bg(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.12)
}

/// Neutral hover fill for list rows.
pub(crate) fn hover_bg(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.04)
}

/// Keep the selected row visibly selected while hovering.
pub(crate) fn selection_hover_bg(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.16)
}
