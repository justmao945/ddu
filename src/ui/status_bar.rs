//! Bottom status strips: one per column, so the panel dividers run
//! from the title bar to the window bottom edge.
//!
//! * [`render_sidebar`] — under the session panel: sidebar toggle at
//!   the left, project creation at the right.
//! * [`render_center`] — under the terminal; adopts the toggles whose
//!   panel is currently hidden so they never vanish with their panel.
//! * [`render_diff`] — under the changes panel: diff toggle at the
//!   right.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::*;
use gpui_kit::*;

use super::AppIcon;
use crate::app::AppView;

pub(crate) fn render_sidebar(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    StatusBar::new()
        .left(h_flex().items_center().child(toggle_sessions(this, cx)))
        .right(
            h_flex().items_center().child(
                Button::new("add-project")
                    .icon(AppIcon::FolderPlus)
                    .ghost()
                    .small()
                    .tab_stop(false)
                    .tooltip("Add project (⌘O)")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_project(window, cx);
                    })),
            ),
        )
}

pub(crate) fn render_center(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let mut bar = StatusBar::new();
    // A hidden panel's toggle moves here so it stays reachable.
    if !this.show_sessions {
        bar = bar.left(h_flex().items_center().child(toggle_sessions(this, cx)));
    }
    if !this.show_diff {
        bar = bar.right(h_flex().items_center().child(toggle_diff(this, cx)));
    }
    // All panels open: the strip would otherwise render as an empty
    // sliver below the terminal. Give it the current run's identity
    // (kind + title), like the bottom of a real terminal.
    if this.show_sessions && this.show_diff {
        let label = current_run_label(this);
        if let Some(label) = label {
            bar = bar.left(
                h_flex()
                    .items_center()
                    .gap_1_5()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Icon::new(IconName::SquareTerminal).text_color(cx.theme().muted_foreground),
                    )
                    .child(label),
            );
        }
    }
    bar
}

/// The live session row's title (agents adopt the PTY's OSC title —
/// already kind-prefixed, e.g. "omp · finished"); `None` when no
/// session exists or the title is empty.
fn current_run_label(this: &AppView) -> Option<String> {
    let s = this.current_session()?;
    let title = s.title.trim();
    if title.is_empty() {
        return None;
    }
    // Live agents announce their own title via OSC (e.g. "omp · ddu");
    // a bare shell shows its kind.
    if s.is_agent() {
        Some(title.to_string())
    } else {
        Some(format!("{} · {title}", s.kind))
    }
}

pub(crate) fn render_diff(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    StatusBar::new().right(h_flex().items_center().child(toggle_diff(this, cx)))
}

fn toggle_sessions(this: &AppView, cx: &mut Context<AppView>) -> Button {
    Button::new("toggle-sessions")
        .tab_stop(false)
        .icon(if this.show_sessions {
            IconName::PanelLeftClose
        } else {
            IconName::PanelLeftOpen
        })
        .ghost()
        .small()
        .tooltip("Toggle sidebar (⌘B)")
        .on_click(cx.listener(|this, _, _, cx| this.toggle_sessions(cx)))
}

fn toggle_diff(this: &AppView, cx: &mut Context<AppView>) -> Button {
    Button::new("toggle-diff")
        .tab_stop(false)
        .icon(if this.show_diff {
            IconName::PanelRightClose
        } else {
            IconName::PanelRightOpen
        })
        .ghost()
        .small()
        .tooltip("Toggle changes (⌘R)")
        .on_click(cx.listener(|this, _, _, cx| this.toggle_diff(cx)))
}
