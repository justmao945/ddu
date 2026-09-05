//! Bottom status bar: dock toggles only (sessions left, changes right).
//! Zed-style - panel toggles live in the bottom strip.

use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::*;

use crate::app::AppView;

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    StatusBar::new()
        .left(
            h_flex()
                .items_center()
                .gap_1()
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_sessions = !this.show_sessions;
                            cx.notify();
                        })),
                ),
        )
        .right(
            h_flex()
                .items_center()
                .gap_1()
                .px_1()
                .child(
                    Button::new("toggle-diff")
                        .icon(if this.show_diff {
                            IconName::PanelRightClose
                        } else {
                            IconName::PanelRightOpen
                        })
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_diff = !this.show_diff;
                            cx.notify();
                        })),
                ),
        )
}
