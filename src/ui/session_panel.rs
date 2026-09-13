//! Left pane: project tree. Level 1 = project folders (`+` quick-add,
//! `...` menu for all launchers and project ops), level 2 = that
//! project's sessions as two-line rows: kind badge + live title, and a
//! meta line with run duration (plus a spinner while an agent works).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{hover_bg, row_px, scaled, selection_bg};
use super::panel_view;
use crate::app::AppView;
use crate::session::{AgentSession, AgentStatus};

/// Height of a two-line session row.
fn session_row_px() -> f32 {
    scaled(40.)
}

/// Update a shared hover slot from one element's `on_hover` callback,
/// returning whether it changed (i.e. whether a repaint is needed).
///
/// gpui's hover set is inclusive, but mouse listeners bubble in REVERSE
/// paint order: moving the pointer from row A to row B fires B's enter
/// *before* A's leave. A leave that cleared the slot unconditionally
/// would stomp the fresh entry (and B's action button, mounted from the
/// slot, would never appear when moving down the list). So a leave only
/// clears the slot while it still names the leaving row.
fn toggle_hover<T: Copy + PartialEq>(slot: &mut Option<T>, hovering: bool, target: T) -> bool {
    if hovering {
        if *slot == Some(target) {
            return false;
        }
        *slot = Some(target);
        true
    } else if *slot == Some(target) {
        *slot = None;
        true
    } else {
        false
    }
}

/// The panel's layout box: one definition, read by the panel's own root
/// element and by the shell's cached mount (`panel_view!` explains why
/// the composer has to state it).
pub(crate) fn root_style() -> StyleRefinement {
    StyleRefinement::default()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .overflow_hidden()
}

panel_view!(
    /// The project/session tree plus the changes-file tree below it.
    PanelView,
    render
);

pub(crate) fn render(
    this: &AppView,
    _window: &mut Window,
    cx: &mut Context<AppView>,
) -> AnyElement {
    // Two layers split by a draggable divider: the project/session
    // tree on top (takes the leftover height), the diff file tree
    // below (capped height, collapsible via the status-strip button).
    // Project creation lives in the status strip below (see
    // `status_bar`).
    let mut root = div();
    *root.style() = root_style();
    root.bg(cx.theme().sidebar)
        // Clicking anywhere in the panel moves focus off the terminal,
        // so its block cursor turns hollow.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.window_focus.focus(window, cx)),
        )
        .child(
            v_resizable("sidebar-split")
                .with_state(&this.sidebar_split_state)
                // Project tree: flexes to whatever the diff-tree layer
                // leaves.
                .child(
                    resizable_panel().child(
                        // A flex column, not a plain `div()`: the tree
                        // below is a `flex_1` item, and only a flex
                        // parent actually constrains it to the panel
                        // height — otherwise it grew to its content and
                        // a long session list was clipped instead of
                        // scrolling.
                        v_flex()
                            .relative()
                            .size_full()
                            .min_h_0()
                            .overflow_hidden()
                            // The scrollbar overlay lives on this
                            // wrapper, NOT on the tracked element: an
                            // absolutely positioned overlay is counted
                            // as scrolled content, and with the
                            // wrapper's padding measured a second time
                            // it left the tree a phantom ~16px of
                            // scroll range — a scrollbar over a list
                            // that does not overflow. Same shape as the
                            // diff tree's host below.
                            .vertical_scrollbar(&this.sessions_scroll)
                            .child(tree(this, cx)),
                    ),
                )
                // Diff tree: sized + `flex_none` so it holds its
                // height while the tree above absorbs the rest;
                // `visible` keeps the splitter's panel indices stable
                // across toggles.
                .child(
                    resizable_panel()
                        .size(
                            this.diff_tree_height_seed
                                .unwrap_or(px(super::diff_tree::tree_default_h())),
                        )
                        .size_range(
                            px(super::diff_tree::tree_min_h())..px(super::diff_tree::tree_max_h()),
                        )
                        .flex_none()
                        .visible(this.show_diff_tree)
                        .child(super::diff_tree::render(this, cx).into_any_element()),
                ),
        )
        .into_any_element()
}

fn tree(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let radius = cx.theme().radius;
    let hov_bg = hover_bg(cx);
    let fg = cx.theme().foreground;
    // The `+` button spawns the configured default — say which, and
    // its global shortcut.
    let cfg = cx.global::<crate::config::Config>();
    let quick_tooltip: SharedString = format!(
        "New {} ({})",
        cfg.label_for(&cfg.new_session.kind),
        crate::app::accel_hint("N")
    )
    .into();
    v_flex()
        .id("project-tree")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&this.sessions_scroll)
        .p_2()
        .gap_0p5()
        .children((0..this.projects.len()).map(move |p| {
            let project = &this.projects[p];
            let expanded = this.expanded.get(p).copied().unwrap_or(true);
            let active_project = p == this.current_project;
            let chevron = if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };

            v_flex()
                .flex_shrink_0()
                .gap_0p5()
                // ── level 1: project row ──
                .child(
                    div()
                        .id(("project-row", p))
                        // Accessibility: the row *is* the disclosure
                        // control for its sessions (click toggles the
                        // layer), so it is a tree item that reports
                        // whether that layer is open.
                        .role(Role::TreeItem)
                        .aria_expanded(expanded)
                        .aria_label(SharedString::from(project.name.clone()))
                        .h(px(row_px()))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .rounded(radius)
                        .cursor_pointer()
                        .hover(move |el| el.bg(hov_bg))
                        .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                            if toggle_hover(&mut this.hovered_project, *hovering, p) {
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
                                        .accessibility_label(quick_tooltip.clone())
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
                                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            this.select_session(p, 0, _window, cx);
                                            this.spawn_session(_window, cx);
                                        })),
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
                                .h(px(row_px()))
                                .flex()
                                .items_center()
                                .pl(px(scaled(18.)))
                                .ml_1()
                                .text_xs()
                                .text_color(fg.opacity(0.35))
                                .child("No session yet"),
                        )
                    } else {
                        el.children(
                            project
                                .sessions
                                .iter()
                                .enumerate()
                                .map(|(six, s)| session_row(this, p, six, s, active_project, cx)),
                        )
                    }
                })
        }))
        // Empty workspace (every project removed): say how to add one.
        .when(this.projects.is_empty(), |el| {
            el.child(
                div()
                    .p_2()
                    .text_xs()
                    .text_color(fg.opacity(0.35))
                    .child(format!(
                        "No projects — add one with {} or the folder button below.",
                        crate::app::accel_hint("O")
                    )),
            )
        })
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
    let active_hover_bg = super::selection_hover_bg(cx);
    let title = session_title(s, cx);
    div()
        .id(element_id)
        // Accessibility: a row is one tree item (the sidebar *is* a tree:
        // project rows nest their sessions), named after the title a screen
        // reader would otherwise have to guess from the painted glyphs, and
        // marked selected when it is the session on screen. The role has to
        // be TreeItem to read as a row on macOS — ListItem maps to a bare
        // group there.
        .role(Role::TreeItem)
        .aria_selected(active)
        .aria_label(SharedString::from(format!("{title} — {}", meta_label(s))))
        .h(px(session_row_px()))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .pl(px(scaled(18.)))
        .pr_2()
        .ml_1()
        .rounded(radius)
        .cursor_pointer()
        .map(|el| if active { el.bg(active_bg) } else { el })
        .hover(move |el| {
            if active {
                el.bg(active_hover_bg)
            } else {
                el.bg(hov_bg)
            }
        })
        .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
            if toggle_hover(&mut this.hovered_session, *hovering, (p, six)) {
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
                        .child(title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .map(|el| match &s.status {
                            AgentStatus::Error(_) | AgentStatus::Done(1..) => {
                                el.text_color(cx.theme().red.opacity(0.8))
                            }
                            _ => el.text_color(fg.opacity(0.45)),
                        })
                        .child(meta_label(s)),
                ),
        )
        .when(this.hovered_session == Some((p, six)), |el| {
            el.child(
                Button::new(SharedString::from(format!("close-{p}-{six}")))
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .tab_stop(false)
                    .tooltip(format!("Close session ({})", crate::app::accel_hint("W")))
                    .accessibility_label(format!(
                        "Close session ({})",
                        crate::app::accel_hint("W")
                    ))
                    // Stop the mouse-down so the row's own click
                    // synthesis never sees this press (see quick-add).
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.request_close_session(p, six, window, cx);
                    })),
            )
        })
}

/// Live row title: agents adopt the PTY's OSC title once they set one;
/// shells keep their program basename (`zsh`).
fn session_title(s: &AgentSession, cx: &Context<AppView>) -> String {
    if s.is_agent()
        && let Some(term) = &s.term
        && let Some(title) = term.read(cx).title()
        && !title.trim().is_empty()
    {
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
            .size(px(scaled(18.)))
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
    let tint = crate::ui::agent_tint(s.kind.as_str(), cx);
    let path = match s.kind.as_str() {
        "claude" => "icons/claude.svg",
        "codex" => "icons/openai.svg",
        _ => "icons/omp.svg",
    };
    cell(
        svg()
            .path(path)
            .size(px(scaled(15.)))
            .flex_shrink_0()
            .text_color(tint)
            .into_any_element(),
    )
}

/// The `...` dropdown on a project row: every launcher plus project ops.
fn more_menu(_this: &AppView, p: usize, cx: &mut Context<AppView>) -> impl IntoElement {
    let cfg = cx.global::<crate::config::Config>().clone();
    let view = cx.weak_entity();
    let new_session_default = cfg.new_session.kind.clone();
    let build_menu =
        move |menu: PopupMenu, _window: &mut Window, _cx: &mut Context<'_, PopupMenu>| {
            // Checkmarks on the right keep the left column free for brand
            // icons (stock Left side would swap the icon for a check).
            let mut m = menu.check_side(Side::Right);
            for kind in cfg.agent_menu() {
                let label = cfg.label_for(&kind);
                let default = kind == new_session_default;
                let kind_click = kind.clone();
                let kind_row = kind.clone();
                let view_click = view.clone();
                m = m.item(
                    PopupMenuItem::element(move |_, cx| {
                        crate::ui::agent_menu_row(&kind_row, label.clone(), cx)
                    })
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
                PopupMenuItem::new("Remove Project")
                    .icon(Icon::new(IconName::CircleX))
                    .on_click({
                        let view = view.clone();
                        move |_, window, cx| {
                            let view = view.clone();
                            window
                                .spawn(cx, {
                                    let view = view.clone();
                                    async move |cx| {
                                        let _ = view.update_in(cx, |v, window, cx| {
                                            v.request_remove_project(p, window, cx);
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
        .accessibility_label("Project actions")
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

#[cfg(test)]
mod tests {
    use super::toggle_hover;

    /// Moving down the list: the entered row reports hover first, the
    /// row being left reports its leave afterwards (reverse paint
    /// order). The stale leave must not clear the fresh slot — that is
    /// exactly how a row lost its Close button.
    #[test]
    fn stale_leave_does_not_clear_a_fresh_hover_slot() {
        let mut slot: Option<(usize, usize)> = None;
        assert!(toggle_hover(&mut slot, true, (0, 2)));
        assert!(!toggle_hover(&mut slot, false, (0, 0)), "stale leave");
        assert_eq!(slot, Some((0, 2)), "the entered row keeps the slot");

        // A repeated enter changes nothing.
        assert!(!toggle_hover(&mut slot, true, (0, 2)));
        // The row's own leave does clear it.
        assert!(toggle_hover(&mut slot, false, (0, 2)));
        assert_eq!(slot, None);
        // Leaving twice (or leaving a row that never had it) is a no-op.
        assert!(!toggle_hover(&mut slot, false, (0, 2)));
    }

    /// The sidebar's scroll range must match the list, not the panel's
    /// own padding: a handful of rows that fit the panel scrolls by
    /// zero (a scrollbar there is the bug), while a list taller than
    /// the panel scrolls by the real overflow instead of growing past
    /// the panel and being clipped.
    #[test]
    fn session_tree_scroll_range_follows_the_list() {
        use crate::app::AppView;
        use crate::config::{Config, ProjectConfig, SavedSession, State};
        use gpui_kit::base::ScrollbarMode;
        use gpui_kit::component::theme::Theme;
        use gpui_kit::{Entity, TestAppContext, gpui};

        fn saved_agent() -> SavedSession {
            SavedSession {
                kind: "omp".into(),
                title: "omp".into(),
                resume: None,
                live: Some(false),
                selected_file: None,
                closed_dirs: vec![],
                tree_height: None,
                view_mode: None,
                tree_filter: None,
}
        }

        /// Build a window whose only project holds `sessions` finished
        /// agent rows (no PTYs are spawned) and report the tree's
        /// maximum scroll offset.
        fn scroll_range(cx: &mut TestAppContext, sessions: usize) -> f32 {
            cx.update(|cx| {
                cx.set_global(Config::default());
                cx.set_global(crate::config::LoadWarnings(vec![]));
                cx.set_global(State {
                    projects: Some(vec![ProjectConfig {
                        name: "p".into(),
                        path: std::env::temp_dir().join("ddu-sidebar-test"),
                        expanded: true,
                        sessions: (0..sessions).map(|_| saved_agent()).collect(),
                    }]),
                    ..Default::default()
                });
            });
            let (view, vcx): (Entity<AppView>, _) =
                cx.add_window_view(|window, cx| AppView::new(window, cx));
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            f32::from(vcx.update(|_, cx| view.read(cx).sessions_scroll.max_offset().y))
        }

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("sidebar_scroll_range"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    Theme::set_scrollbar_mode(ScrollbarMode::Hover, cx);
                });
                // Three rows sit far inside any panel: nothing to scroll.
                assert_eq!(scroll_range(cx, 3), 0.);
                // Forty rows overflow it: the tree scrolls by the overflow.
                assert!(scroll_range(cx, 40) > 0.);
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
