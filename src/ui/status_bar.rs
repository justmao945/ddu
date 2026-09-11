//! Bottom status strips.
//!
//! * [`render_sidebar`] — under the session panel: sidebar toggle at
//!   the left, project creation at the right. The sidebar owning a
//!   strip keeps the left resize divider running to the window's
//!   bottom edge.
//! * [`render_center`] — ONE unified bar under the center + changes
//!   region (below the resizable split, so the right divider stops at
//!   the bar's top edge). Adopts the toggles whose panel is currently
//!   hidden so they never vanish with their panel.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::*;
use gpui_kit::*;

use super::AppIcon;
use crate::app::AppView;

pub(crate) fn render_sidebar(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    StatusBar::new()
        .left(
            h_flex()
                .items_center()
                .gap_0p5()
                .child(toggle_sessions(this, cx))
                .child(toggle_diff_tree(this, cx)),
        )
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
        bar = bar.left(
            h_flex()
                .items_center()
                .gap_0p5()
                .child(toggle_sessions(this, cx))
                .child(toggle_diff_tree(this, cx)),
        );
    }
    bar.right(h_flex().items_center().child(toggle_diff(this, cx)))
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
        .on_click(cx.listener(|this, _, window, cx| this.toggle_diff(window, cx)))
}

/// Collapse/expand the diff file tree layer under the project tree.
fn toggle_diff_tree(this: &AppView, cx: &mut Context<AppView>) -> Button {
    Button::new("toggle-diff-tree")
        .tab_stop(false)
        .icon(if this.show_diff_tree {
            IconName::PanelBottom
        } else {
            IconName::PanelBottomOpen
        })
        .ghost()
        .small()
        .tooltip("Toggle file tree (⌘T)")
        .on_click(cx.listener(|this, _, _, cx| this.toggle_diff_tree(cx)))
}
