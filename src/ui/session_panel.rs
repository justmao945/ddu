//! Left pane: project tree. Level 1 = project folders (`+` quick-add,
//! `...` menu for all launchers and project ops), level 2 = that
//! project's sessions (status dot + title + kind, single line). The
//! panel header is a session-search box; typing filters session rows.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{PANEL_HEADER_PX, hover_bg, meta_text, selection_bg, status_dot, ROW_PX};
use crate::app::AppView;

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    v_flex()
        .h_full()
        .w_full()
        .bg(cx.theme().sidebar)
        .child(
            div()
                .h(px(PANEL_HEADER_PX))
                .flex_shrink_0()
                .px_2()
                .flex()
                .items_center()
                .gap_1()
                .child(Input::new(&this.search).small().cleanable(true).flex_1())
                .child(
                    Button::new("add-project")
                        .icon(IconName::Folder)
                        .ghost()
                        .small()
                        .tooltip("Add project…")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_project(window, cx);
                        })),
                ),
        )
        .child(tree(this, cx))
}

fn query(this: &AppView, cx: &Context<AppView>) -> String {
    this.search.read(cx).value().trim().to_lowercase()
}

fn tree(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let radius = cx.theme().radius;
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);
    let fg = cx.theme().foreground;
    let query = query(this, cx);
    let filtering = !query.is_empty();

    v_flex()
        .id("project-tree")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p_2()
        .gap_0p5()
        .children((0..this.projects.len()).filter_map(move |p| {
            let project = &this.projects[p];
            let expanded = this.expanded.get(p).copied().unwrap_or(true);
            let active_project = p == this.current_project;
            let chevron = if expanded { IconName::ChevronDown } else { IconName::ChevronRight };

            // Session rows, filtered by the search query (title or kind).
            let sessions: Vec<_> = project
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    !filtering
                        || s.title.to_lowercase().contains(&query)
                            || s.kind_label_inner().to_lowercase().contains(&query)
                })
                .collect();
            if filtering && sessions.is_empty() {
                return None;
            }

            Some(
                v_flex()
                    .gap_0p5()
                    // ── level 1: project row ──
                    .child(
                        div()
                            .id(("project-row", p))
                            .h(px(ROW_PX))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .rounded(radius)
                            .cursor_pointer()
                            .hover(move |el| if active_project { el } else { el.bg(hov_bg) })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.expanded[p] = !this.expanded[p];
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
                            .child(
                                Button::new(("quick-add", p))
                                    .icon(IconName::Plus)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("New session (default)")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.current_project = p;
                                        this.spawn_session(window, cx);
                                    })),
                            )
                            .child(more_menu(this, p, cx)),
                    )
                    // ── level 2: session rows ──
                    .when(expanded, |el| {
                        el.children(sessions.into_iter().map(|(six, s)| {
                            let active = active_project && six == this.current_session;
                            let dot = status_dot(&s.status, cx);
                            let element_id = SharedString::from(s.id.clone());
                            let kind = s.kind_label_inner();

                            div()
                                .id(element_id)
                                .h(px(ROW_PX))
                                .flex()
                                .items_center()
                                .gap_2()
                                .pl(px(18.))
                                .pr_2()
                                .ml_1()
                                .rounded(radius)
                                .cursor_pointer()
                                .map(|el| if active { el.bg(active_bg) } else { el })
                                .hover(move |el| if active { el } else { el.bg(hov_bg) })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_session(p, six, window, cx);
                                }))
                                .child(
                                    div()
                                        .size(px(7.))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(dot),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(if active { fg } else { fg.opacity(0.85) })
                                        .child(s.title.clone()),
                                )
                                .child(meta_text(kind, cx))
                        }))
                    }),
            )
        }))
}

/// The `...` dropdown on a project row: every launcher plus project ops.
fn more_menu(_this: &AppView, p: usize, cx: &mut Context<AppView>) -> impl IntoElement {
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
                                        v.spawn_session_of(&kind, window, cx)
                                    });
                                }
                            })
                            .detach();
                    }),
            );
        }
        m = m.separator().item(
            PopupMenuItem::new("Remove project").on_click({
                let view = view.clone();
                move |_, window, cx| {
                    let view = view.clone();
                    window
                        .spawn(cx, {
                            let view = view.clone();
                            async move |cx| {
                                let _ = view.update(cx, |v, cx| v.remove_project(p, cx));
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
        .dropdown_menu_with_anchor(Anchor::TopRight, build_menu)
}
