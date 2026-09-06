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
    let resume_id = term.read(cx).resume_id().map(String::from);
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
                        .action(Box::new(crate::app::TermCopy))
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
                        .action(Box::new(crate::app::TermPaste))
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
                                // Hovering the right-edge strip keeps
                                // the overlay thumb up.
                                s.scrollbar_hover_at(event.position, cx);
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
        .when(exited.is_some(), |el| {
            el.child(exited_banner(exited, resume_id, cx))
        })
}

/// In-flow exit strip at the bottom of the terminal: status + resume
/// (when the agent printed its session id) + restart. One compact row,
/// visually level with the status strips.
fn exited_banner(
    exit: Option<i32>,
    resume_id: Option<String>,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let label = match exit {
        Some(0) => "Session finished".to_string(),
        Some(code) => format!("Session exited ({code})"),
        None => String::new(),
    };
    h_flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .py_1()
        .gap_2()
        .text_sm()
        .text_color(cx.theme().foreground.opacity(0.8))
        .child(label)
        .when_some(resume_id, |el, _id| {
            el.child(
                Button::new("resume-session")
                    .label("Resume")
                    .ghost()
                    .with_size(gpui_kit::component::Size::XSmall)
                    .tab_stop(false)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.resume_current_session(window, cx);
                    })),
            )
        })
        .child(
            Button::new("restart-session")
                .label("Restart")
                .ghost()
                .with_size(gpui_kit::component::Size::XSmall)
                .tab_stop(false)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.restart_current_session(window, cx);
                })),
        )
}

/// Empty center pane: no session in this project. Tracks a handle so the
/// window keeps a focus chain and global shortcuts keep working.
fn empty_state(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let has_session = this.current_session().is_some();
    let error = this.current_session().and_then(|s| match &s.status {
        crate::session::AgentStatus::Error(error) => Some(error.clone()),
        _ => None,
    });
    // A restored row (agent finished before the app closed) still offers
    // its Resume action from here — the PTY never came back.
    let can_resume = this
        .current_session()
        .and_then(|s| s.cmd.resume.clone())
        .is_some();
    let resumed_label = this
        .current_session()
        .map(|s| format!("{} · finished", s.kind))
        .unwrap_or_default();
    // With no session anywhere, a New Session button can't know which
    // project to spawn into — point at the sidebar instead.
    let nowhere = this.projects.iter().all(|p| p.sessions.is_empty());
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
            "Unable to start session".to_string()
        } else if can_resume {
            format!("{resumed_label} — resume the conversation?")
        } else if has_session {
            // Restored row with no session id to resume — restart it.
            resumed_label
        } else if nowhere {
            "No sessions yet — add one from a project in the sidebar".to_string()
        } else {
            "No sessions in this project".to_string()
        })
        .when_some(error.clone(), |el, error| {
            el.child(div().max_w(px(480.)).child(error))
        })
        .children((0..1).filter_map(move |_| {
            if nowhere && error.is_none() && !has_session {
                return None;
            }
            let (label, action) = if error.is_some() {
                ("Retry", 0)
            } else if can_resume {
                ("Resume", 1)
            } else if has_session {
                ("Restart", 2)
            } else {
                ("New session", 3)
            };
            Some(
                Button::new("empty-session-action")
                    .tab_stop(false)
                    .label(label)
                    .on_click(cx.listener(move |this, _, window, cx| match action {
                        0 | 2 => this.restart_current_session(window, cx),
                        1 => this.resume_current_session(window, cx),
                        _ => this.spawn_session(window, cx),
                    })),
            )
        }))
}
