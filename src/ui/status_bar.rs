//! Bottom status bar: just the two panel-toggle buttons.

use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::*;

use crate::app::AppView;

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    StatusBar::new()
        .left(
            h_flex().items_center().px_1().child(
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
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sessions(cx))),
            ),
        )
        .right(
            h_flex().items_center().px_1().child(
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
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_diff(cx))),
            ),
        )
}
