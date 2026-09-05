//! Center pane: the live agent terminal. A focus-tracked surface that
//! forwards keystrokes and scroll events into the PTY and paints the
//! grid via [`crate::terminal::element::TerminalElement`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::AppView;
use crate::terminal::TermSession;

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .h_full()
        .flex_1()
        .min_w_0()
        .bg(cx.theme().background)
        .child(match this.current_term() {
            Some(term) => surface(term, cx).into_any_element(),
            None => empty_state(this, cx).into_any_element(),
        })
}

/// The focus-tracked terminal surface for one live session.
fn surface(term: Entity<TermSession>, cx: &mut Context<AppView>) -> impl IntoElement {
    let focus = term.read(cx).focus.clone();
    let exited = term.read(cx).exit();
    let weak = term.downgrade();

    div()
        .id("terminal-surface")
        .relative()
        .h_full()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        .overflow_hidden()
        .text_color(cx.theme().foreground)
        .track_focus(&focus)
        .key_context("Terminal")
        .on_click(cx.listener({
            let focus = focus.clone();
            move |_, _, window, cx| {
                focus.focus(window, cx);
            }
        }))
        .on_key_down(cx.listener({
            let weak = weak.clone();
            move |_, event: &KeyDownEvent, _, cx| {
                // ⌘-chords stay reserved for app actions.
                let Some(bytes) = TermSession::encode_keystroke(&event.keystroke) else { return };
                if let Some(term) = weak.upgrade() {
                    term.update(cx, |s, _| {
                        s.write(&bytes);
                        s.scroll_to_bottom();
                    });
                    cx.stop_propagation();
                }
            }
        }))
        .on_scroll_wheel(cx.listener({
            let weak = weak.clone();
            move |_, event: &ScrollWheelEvent, _, cx| {
                let lines = match event.delta {
                    ScrollDelta::Lines(p) => p.y,
                    ScrollDelta::Pixels(p) => f32::from(p.y / px(40.)),
                };
                if let Some(term) = weak.upgrade() {
                    term.update(cx, |s, _| s.scroll(lines as i32));
                }
                cx.stop_propagation();
            }
        }))
        .child(
            div()
                .h_full()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(TermSession::element(weak, focus)),
        )
        .when(exited.is_some(), |el| el.child(exited_banner(exited, cx)))
}

/// In-flow exit strip at the bottom of the terminal: status + restart
/// (same command, fresh PTY). Lives inside the flex column so it can
/// never be clipped like the old absolutely-positioned chip.
fn exited_banner(exit: Option<i32>, cx: &mut Context<AppView>) -> impl IntoElement {
    let label = match exit {
        Some(0) => "agent finished".to_string(),
        Some(code) => format!("agent exited ({code})"),
        None => String::new(),
    };
    h_flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap_2()
        .border_t_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .px_3()
        .py_1p5()
        .text_sm()
        .text_color(cx.theme().foreground.opacity(0.8))
        .child(label)
        .child(
            Button::new("restart-session")
                .label("Restart")
                .ghost()
                .small()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.restart_current_session(window, cx);
                })),
        )
}

/// Empty center pane: no session in this project. Tracks a handle so the
/// window keeps a focus chain and global shortcuts keep working.
fn empty_state(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .track_focus(&this.window_focus)
        .text_color(cx.theme().foreground.opacity(0.5))
        .child("No agent session — start one with ⌘T")
}
