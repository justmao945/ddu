//! Left pane: project tree. Level 1 = project folders (`+` quick-add,
//! `...` menu for all launchers and project ops), level 2 = that
//! project's sessions as two-line rows: kind badge + live title, and a
//! meta line with run duration (plus a spinner while an agent works).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{hover_bg, selection_bg, ROW_PX};
use crate::app::AppView;
use crate::session::{AgentSession, AgentStatus};

/// Height of a two-line session row.
const SESSION_ROW_PX: f32 = 40.;

/// PTY output within this window ⇒ the agent is actively working.
const ACTIVE_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    // No panel header: the project tree fills the column; project
    // creation lives in the status strip below (see `status_bar`).
    v_flex()
        .h_full()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .bg(cx.theme().sidebar)
        .child(tree(this, cx))
}

fn tree(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let radius = cx.theme().radius;
    let hov_bg = hover_bg(cx);
    let fg = cx.theme().foreground;
    // The `+` button spawns the configured default — say which.
    let cfg = cx.global::<crate::config::Config>();
    let quick_tooltip: SharedString =
        format!("New {}", cfg.label_for(&cfg.new_session.kind)).into();
    v_flex()
        .id("project-tree")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p_2()
        .gap_0p5()
        .children((0..this.projects.len()).map(move |p| {
            let project = &this.projects[p];
            let expanded = this.expanded.get(p).copied().unwrap_or(true);
            let active_project = p == this.current_project;
            let chevron = if expanded { IconName::ChevronDown } else { IconName::ChevronRight };

            v_flex()
                    .flex_shrink_0()
                    .gap_0p5()
                    // ── level 1: project row ──
                    .child(
                        div()
                            .id(("project-row", p))
                            .h(px(ROW_PX))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .rounded(radius)
                            .cursor_pointer()
                            .hover(move |el| el.bg(hov_bg))
                            .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                                let next = if *hovering { Some(p) } else { None };
                                if this.hovered_project != next {
                                    this.hovered_project = next;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.expanded[p] = !this.expanded[p];
                                this.persist(cx);
                                cx.notify();
                            }))
                            .child(
                                Icon::new(chevron)
                                    .with_size(gpui_kit::component::Size::XSmall)
                                    .text_color(fg.opacity(0.55)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_medium()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(if active_project { fg } else { fg.opacity(0.85) })
                                    .child(project.name.clone()),
                            )
                            // Ops appear while the row is hovered — or while
                            // this row's `...` menu is open: the popup
                            // occludes the row, so hover would drop and
                            // unmount the trigger mid-interaction.
                            .when(
                                this.hovered_project == Some(p) || this.menu_project == Some(p),
                                |el| {
                                    el.child(
                                        Button::new(("quick-add", p))
                                            .icon(IconName::Plus)
                                            .ghost()
                                            .xsmall()
                                            .tab_stop(false)
                                            .tooltip(quick_tooltip.clone())
                                            // gpui synthesizes a click for
                                            // EVERY hitbox under the pointer;
                                            // stopping the click event alone
                                            // doesn't stop the row's own click
                                            // synthesis. Kill the mouse-down
                                            // instead, like BasePopover
                                            // triggers do, so the row never
                                            // records a press and its
                                            // expand/collapse on_click stays
                                            // quiet.
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                |_, _, cx| cx.stop_propagation(),
                                            )
                                            .on_click(cx.listener(
                                                move |this, _, _window, cx| {
                                                    this.select_session(p, 0, _window, cx);
                                                    this.spawn_session(_window, cx);
                                                },
                                            )),
                                    )
                                    .child(more_menu(this, p, cx))
                                },
                            ),
                    )
                    // ── level 2: session rows ──
                    .when(expanded, |el| {
                        if project.sessions.is_empty() {
                            el.child(
                                div()
                                    .h(px(ROW_PX))
                                    .flex()
                                    .items_center()
                                    .pl(px(18.))
                                    .ml_1()
                                    .text_xs()
                                    .text_color(fg.opacity(0.35))
                                    .child("No session yet"),
                            )
                        } else {
                            el.children(project.sessions.iter().enumerate().map(|(six, s)| {
                                session_row(this, p, six, s, active_project, cx)
                            }))
                        }
                    })
        }))
}

fn session_row(
    this: &AppView,
    p: usize,
    six: usize,
    s: &AgentSession,
    active_project: bool,
    cx: &mut Context<AppView>,
) -> impl IntoElement + use<> {
    let radius = cx.theme().radius;
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);
    let fg = cx.theme().foreground;
    let active = active_project && six == this.current_session;
    let element_id = SharedString::from(s.id.clone());
    // Working state tracks live PTY output, not the (long-lived) process:
    // an agent idling at its prompt must not look busy.
    let working = s.status.is_running()
        && s.is_agent()
        && s.term
            .as_ref()
            .is_some_and(|t| t.read(cx).active_within(ACTIVE_WINDOW));
    let active_hover_bg = super::selection_hover_bg(cx);
    let title = session_title(s, cx);
    div()
        .id(element_id)
        .h(px(SESSION_ROW_PX))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .pl(px(18.))
        .pr_2()
        .ml_1()
        .rounded(radius)
        .cursor_pointer()
        .map(|el| if active { el.bg(active_bg) } else { el })
        .hover(move |el| if active { el.bg(active_hover_bg) } else { el.bg(hov_bg) })
        .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
            let next = if *hovering { Some((p, six)) } else { None };
            if this.hovered_session != next {
                this.hovered_session = next;
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.select_session(p, six, window, cx);
        }))
        .child(kind_icon(s, cx))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(if active { fg } else { fg.opacity(0.85) })
                        .child(title.clone())
                )
                .child(
                    div()
                        .text_xs()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .map(|el| match &s.status {
                            AgentStatus::Error(_) | AgentStatus::Done(1..) => el.text_color(cx.theme().red.opacity(0.8)),
                            _ => el.text_color(fg.opacity(0.45)),
                        })
                        .child(meta_label(s)),
                ),
        )
        .when(working, |el| {
            // Activity pulse: a small accent dot, Zed-style — the spinner
            // read as perpetual motion for background work.
            el.child(
                div()
                    .flex_shrink_0()
                    .size(px(6.))
                    .rounded_full()
                    .bg(cx.theme().accent),
            )
        })
        .when(
            this.hovered_session == Some((p, six)),
            |el| {
                el.child(
                    Button::new(SharedString::from(format!("close-{p}-{six}")))
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tab_stop(false)
                        .tooltip("Close session")
                        // Stop the mouse-down so the row's own click
                        // synthesis never sees this press (see quick-add).
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            |_, _, cx| cx.stop_propagation(),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request_close_session(p, six, window, cx);
                        })),
                )
            },
        )
}

/// Live row title: agents adopt the PTY's OSC title once they set one;
/// shells keep their program basename (`zsh`).
fn session_title(s: &AgentSession, cx: &Context<AppView>) -> String {
    if s.is_agent()
        && let Some(term) = &s.term
            && let Some(title) = term.read(cx).title()
                && !title.trim().is_empty() {
                    return title.trim().to_string();
                }
    s.title.clone()
}

/// Second line: run duration, with the terminal state for exited runs.
fn meta_label(s: &AgentSession) -> String {
    match s.status {
        AgentStatus::Running => s.elapsed_label(),
        AgentStatus::Done(0) => format!("Finished · {}", s.elapsed_label()),
        AgentStatus::Done(code) => format!("Exit {code} · {}", s.elapsed_label()),
        AgentStatus::Error(_) => format!("Failed · {}", s.elapsed_label()),
    }
}

/// Kind badge: the shell terminal icon, or the agent's brand mark
/// (monochrome SVGs tinted per brand, like Zed's icons).
fn kind_icon(s: &AgentSession, cx: &Context<AppView>) -> Div {
    let fg = cx.theme().foreground;
    let cell = |child: AnyElement| {
        div()
            .size(px(18.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(child)
    };
    if !s.is_agent() || !matches!(s.kind.as_str(), "claude" | "codex" | "omp") {
        return cell(
            Icon::new(IconName::SquareTerminal)
                .with_size(gpui_kit::component::Size::Small)
                .text_color(fg.opacity(0.6))
                .into_any_element(),
        );
    }
    let (path, tint) = match s.kind.as_str() {
        "claude" => ("icons/claude.svg", hsla(15. / 360., 0.64, 0.5, 1.)),
        "codex" => ("icons/openai.svg", fg.opacity(0.85)),
        "omp" => ("icons/omp.svg", hsla(258. / 360., 0.7, 0.55, 1.)),
        _ => ("icons/omp.svg", fg.opacity(0.5)),
    };
    cell(
        svg()
            .path(path)
            .size(px(15.))
            .flex_shrink_0()
            .text_color(tint)
            .into_any_element(),
    )
}

/// Menu-leading icon for a launcher kind: brand marks for the builtins,
/// a terminal glyph for shells, a generic bot for custom presets.
fn kind_menu_icon(kind: &str) -> Icon {
    match kind {
        "terminal" => Icon::new(IconName::SquareTerminal),
        "claude" => Icon::default().path("icons/claude.svg"),
        "codex" => Icon::default().path("icons/openai.svg"),
        "omp" => Icon::default().path("icons/omp.svg"),
        _ => Icon::new(IconName::Bot),
    }
}

/// The `...` dropdown on a project row: every launcher plus project ops.
fn more_menu(this: &AppView, p: usize, cx: &mut Context<AppView>) -> impl IntoElement {
    let can_remove = this.projects.len() > 1;
    let has_running = this.projects[p].sessions.iter().any(|s| s.status.is_running());
    let cfg = cx.global::<crate::config::Config>().clone();
    let view = cx.weak_entity();
    let new_session_default = cfg.new_session.kind.clone();
    let build_menu = move |menu: PopupMenu, _window: &mut Window, _cx: &mut Context<'_, PopupMenu>| {
        let mut m = menu.label("New session");
        for kind in cfg.agent_menu() {
            let label = cfg.label_for(&kind);
            let default = kind == new_session_default;
            let kind_click = kind.clone();
            let view_click = view.clone();
            m = m.item(
                PopupMenuItem::new(label)
                    .icon(kind_menu_icon(&kind))
                    .checked(default)
                    .on_click(move |_, window, cx| {
                        let kind = kind_click.clone();
                        let view = view_click.clone();
                        window
                            .spawn(cx, {
                                let kind = kind.clone();
                                let view = view.clone();
                                async move |cx| {
                                    let _ = view.update_in(cx, |v, window, cx| {
                                        v.select_session(p, 0, window, cx);
                                        v.spawn_session_of(&kind, window, cx)
                                    });
                                }
                            })
                            .detach();
                    }),
            );
        }
        m = m.separator().item(
            PopupMenuItem::new(if has_running { "Close sessions before removing" } else { "Remove project" }).icon(Icon::new(IconName::CircleX)).disabled(!can_remove || has_running).on_click({
                let view = view.clone();
                move |_, window, cx| {
                    let view = view.clone();
                    window
                        .spawn(cx, {
                            let view = view.clone();
                            async move |cx| {
                                let _ = view.update_in(cx, |v, window, cx| {
                                    v.remove_project(p, cx);
                                    v.select_session(v.current_project, v.current_session, window, cx);
                                });
                            }
                        })
                        .detach();
                }
            }),
        );
        m
    };

    Button::new(("more", p))
        .icon(IconName::Ellipsis)
        .ghost()
        .xsmall()
        .tab_stop(false)
        .dropdown_menu_with_anchor(Anchor::TopRight, build_menu)
        .on_open_change({
            let view = cx.weak_entity();
            move |open, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.menu_project = if *open { Some(p) } else { None };
                    cx.notify();
                });
            }
        })
}
