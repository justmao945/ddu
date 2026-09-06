//! Bottom status strips: one per column, so the panel dividers run
//! from the title bar to the window bottom edge.
//!
//! * [`render_sidebar`] — under the session panel: sidebar toggle at
//!   the left, project creation at the right.
//! * [`render_center`] — under the terminal; adopts the toggles whose
//!   panel is currently hidden so they never vanish with their panel.
//! * [`render_diff`] — under the changes panel: diff toggle at the
//!   right.

use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::*;

use super::AppIcon;
use crate::app::AppView;

pub(crate) fn render_sidebar(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    StatusBar::new()
        .left(h_flex().items_center().child(toggle_sessions(this, cx)))
        .right(h_flex().items_center().child(
            Button::new("add-project")
                .icon(AppIcon::FolderPlus)
                .ghost()
                .small()
                .tab_stop(false)
                .tooltip("Add project…")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.add_project(window, cx);
                })),
        ))
}

pub(crate) fn render_center(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let bar = StatusBar::new();
    // A hidden panel's toggle moves here so it stays reachable.
    let bar = if this.show_sessions {
        bar
    } else {
        bar.left(h_flex().items_center().child(toggle_sessions(this, cx)))
    };
    if this.show_diff {
        bar
    } else {
        bar.right(h_flex().items_center().child(toggle_diff(this, cx)))
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
