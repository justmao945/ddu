//! Center pane: the live agent terminal. A focus-tracked surface that
//! forwards keystrokes and scroll events into the PTY and paints the
//! grid via [`crate::terminal::element::TerminalElement`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::{AppView, TermBacktab, TermCopy, TermPaste, TermTab};
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
                let Some(bytes) = TermSession::encode_keystroke(&event.keystroke) else {
                    return;
                };
                if let Some(term) = weak.upgrade() {
                    term.update(cx, |s, cx| {
                        s.write(&bytes);
                        s.scroll_to_bottom();
                        cx.emit(crate::terminal::TermEvent::Wakeup);
                    });
                    cx.stop_propagation();
                }
            }
        }))
        .on_scroll_wheel(cx.listener({
            let weak = weak.clone();
            move |_, event: &ScrollWheelEvent, window, cx| {
                let lines = match event.delta {
                    ScrollDelta::Lines(p) => p.y,
                    ScrollDelta::Pixels(p) => p.y / px(40.),
                };
                if let Some(term) = weak.upgrade() {
                    term.update(cx, |s, cx| {
                        // A mouse-tracking child receives wheel reports
                        // (buttons 64/65) instead of viewport scrolls.
                        if !s.mouse_wheel(lines, event.position, &event.modifiers, window, cx) {
                            s.scroll_by(lines, cx);
                        }
                    });
                }
                cx.stop_propagation();
            }
        }))
        // Tab / Shift-Tab / ⌘V reach here via the "Terminal"-scoped
        // keybindings (registered in `AppView::new`); they must become
        // PTY bytes, not gpui-component's global focus cycling.
        .on_action(cx.listener({
            let weak = weak.clone();
            move |_, _: &TermTab, _, cx| {
                if let Some(term) = weak.upgrade() {
                    term.update(cx, |s, cx| {
                        s.write(b"\t");
                        s.scroll_to_bottom();
                        cx.emit(crate::terminal::TermEvent::Wakeup);
                    });
                }
            }
        }))
        .on_action(cx.listener({
            let weak = weak.clone();
            move |_, _: &TermBacktab, _, cx| {
                if let Some(term) = weak.upgrade() {
                    term.update(cx, |s, cx| {
                        s.write(b"\x1b[Z");
                        s.scroll_to_bottom();
                        cx.emit(crate::terminal::TermEvent::Wakeup);
                    });
                }
            }
        }))
        .on_action(cx.listener({
            let weak = weak.clone();
            move |_, _: &TermPaste, _, cx| {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return;
                };
                if let Some(term) = weak.upgrade() {
                    term.update(cx, |s, cx| {
                        s.paste_text(&text);
                        cx.emit(crate::terminal::TermEvent::Wakeup);
                    });
                }
            }
        }))
        .context_menu({
            let weak = weak.clone();
            move |menu, _window, cx| {
                // A mouse-tracking child owns right-click — the empty
                // menu suppresses the popup entirely.
                if weak.upgrade().is_some_and(|t| {
                    t.read(cx).mouse_tracking() != crate::terminal::MouseTracking::None
                }) {
                    return menu;
                }
                let has_selection = weak.upgrade().is_some_and(|t| t.read(cx).has_selection());
                let can_paste = cx
                    .read_from_clipboard()
                    .is_some_and(|item| item.text().is_some());
                let copy_term = weak.clone();
                let paste_term = weak.clone();
                menu.item(
                    PopupMenuItem::new("Copy")
                        .disabled(!has_selection)
                        .on_click(move |_, _, cx| {
                            if let Some(t) = copy_term.upgrade() {
                                t.update(cx, |s, cx| {
                                    s.copy_selection(cx);
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new("Paste")
                        .disabled(!can_paste)
                        .on_click(move |_, _, cx| {
                            let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                            else {
                                return;
                            };
                            if let Some(t) = paste_term.upgrade() {
                                t.update(cx, |s, cx| {
                                    s.paste_text(&text);
                                    cx.emit(crate::terminal::TermEvent::Wakeup);
                                });
                            }
                        }),
                )
            }
        })
        .child(
            div()
                .h_full()
                // Mouse text selection: down starts (double-click = semantic
                // word), move grows while the button is held, up settles — a
                // plain click collapses to nothing and clears the wash. The
                // right-edge scrollbar strip intercepts first: thumb drag or
                // track paging instead of selecting.
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener({
                        let weak = weak.clone();
                        move |_, event: &MouseDownEvent, window, cx| {
                            if let Some(term) = weak.upgrade() {
                                term.update(cx, |s, cx| {
                                    // A mouse-tracking child (TUI) gets the press as
                                    // an escape report instead of text selection.
                                    if s.mouse_button(
                                        gpui_kit::MouseButton::Left,
                                        true,
                                        event.position,
                                        &event.modifiers,
                                        window,
                                        cx,
                                    ) {
                                        return;
                                    }
                                    if s.scrollbar_mouse_down(event.position, cx) {
                                        return;
                                    }
                                    if let Some((cell, side)) =
                                        s.cell_at(event.position, window, cx)
                                    {
                                        s.begin_selection(cell, side, event.click_count, cx);
                                    }
                                });
                            }
                        }
                    }),
                )
                .on_mouse_move(cx.listener({
                    let weak = weak.clone();
                    move |_, event: &MouseMoveEvent, window, cx| {
                        if let Some(term) = weak.upgrade() {
                            term.update(cx, |s, cx| {
                                if s.mouse_motion(event.position, &event.modifiers, window, cx) {
                                    return;
                                }
                                if s.scrollbar_mouse_drag(event.position, cx) {
                                    return;
                                }
                                if let Some((cell, side)) = s.cell_at(event.position, window, cx) {
                                    s.grow_selection(cell, side, cx);
                                }
                            });
                        }
                    }
                }))
                .on_mouse_up(
                    gpui_kit::MouseButton::Left,
                    cx.listener({
                        let weak = weak.clone();
                        move |_, event: &MouseUpEvent, window, cx| {
                            if let Some(term) = weak.upgrade() {
                                term.update(cx, |s, cx| {
                                    if s.mouse_button(
                                        gpui_kit::MouseButton::Left,
                                        false,
                                        event.position,
                                        &event.modifiers,
                                        window,
                                        cx,
                                    ) {
                                        return;
                                    }
                                    s.scrollbar_mouse_up();
                                    s.end_selection(cx);
                                });
                            }
                        }
                    }),
                )
                .on_mouse_up_out(
                    gpui_kit::MouseButton::Left,
                    cx.listener({
                        let weak = weak.clone();
                        move |_, event: &MouseUpEvent, window, cx| {
                            if let Some(term) = weak.upgrade() {
                                term.update(cx, |s, cx| {
                                    if s.mouse_button(
                                        gpui_kit::MouseButton::Left,
                                        false,
                                        event.position,
                                        &event.modifiers,
                                        window,
                                        cx,
                                    ) {
                                        return;
                                    }
                                    s.scrollbar_mouse_up();
                                    s.end_selection(cx);
                                });
                            }
                        }
                    }),
                )
                // Right/middle buttons exist only for mouse-tracking children —
                // otherwise right-click opens the context menu and middle is inert.
                .on_mouse_down(
                    gpui_kit::MouseButton::Right,
                    cx.listener({
                        let weak = weak.clone();
                        move |_, event: &MouseDownEvent, window, cx| {
                            if let Some(term) = weak.upgrade() {
                                term.update(cx, |s, cx| {
                                    s.mouse_button(
                                        gpui_kit::MouseButton::Right,
                                        true,
                                        event.position,
                                        &event.modifiers,
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }
                    }),
                )
                .on_mouse_up(
                    gpui_kit::MouseButton::Right,
                    cx.listener({
                        let weak = weak.clone();
                        move |_, event: &MouseUpEvent, window, cx| {
                            if let Some(term) = weak.upgrade() {
                                term.update(cx, |s, cx| {
                                    s.mouse_button(
                                        gpui_kit::MouseButton::Right,
                                        false,
                                        event.position,
                                        &event.modifiers,
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }
                    }),
                )
                .on_mouse_down(
                    gpui_kit::MouseButton::Middle,
                    cx.listener({
                        let weak = weak.clone();
                        move |_, event: &MouseDownEvent, window, cx| {
                            if let Some(term) = weak.upgrade() {
                                term.update(cx, |s, cx| {
                                    s.mouse_button(
                                        gpui_kit::MouseButton::Middle,
                                        true,
                                        event.position,
                                        &event.modifiers,
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }
                    }),
                )
                .on_mouse_up(
                    gpui_kit::MouseButton::Middle,
                    cx.listener({
                        let weak = weak.clone();
                        move |_, event: &MouseUpEvent, window, cx| {
                            if let Some(term) = weak.upgrade() {
                                term.update(cx, |s, cx| {
                                    s.mouse_button(
                                        gpui_kit::MouseButton::Middle,
                                        false,
                                        event.position,
                                        &event.modifiers,
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }
                    }),
                )
                .on_action(cx.listener({
                    let weak = weak.clone();
                    move |_, _: &TermCopy, _, cx| {
                        if let Some(term) = weak.upgrade() {
                            if term.update(cx, |s, cx| s.copy_selection(cx)) {
                                cx.stop_propagation();
                            }
                        }
                    }
                }))
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
        Some(0) => "Session finished".to_string(),
        Some(code) => format!("Session exited ({code})"),
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
                .tab_stop(false)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.restart_current_session(window, cx);
                })),
        )
}

/// Empty center pane: no session in this project. Tracks a handle so the
/// window keeps a focus chain and global shortcuts keep working.
fn empty_state(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let error = this.current_session().and_then(|s| match &s.status {
        crate::session::AgentStatus::Error(error) => Some(error.clone()),
        _ => None,
    });
    v_flex()
        .size_full()
        .p_6()
        .items_center()
        .justify_center()
        .gap_3()
        .track_focus(&this.window_focus)
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(if error.is_some() {
            "Unable to start session"
        } else {
            "No sessions in this project"
        })
        .when_some(error.clone(), |el, error| {
            el.child(div().max_w(px(480.)).child(error))
        })
        .child(
            Button::new("empty-session-action")
                .tab_stop(false)
                .label(if error.is_some() {
                    "Retry"
                } else {
                    "New session"
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if error.is_some() {
                        this.restart_current_session(window, cx);
                    } else {
                        this.spawn_session(window, cx);
                    }
                })),
        )
}
