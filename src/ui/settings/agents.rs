//! Agent launcher pages: builtin agent argument fields and the
//! custom agent command list.

use super::{commit_input, commit_text_field, update_config};
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem};
use gpui_kit::*;

pub(super) fn builtin_agent_groups(cx: &App) -> SettingGroup {
    let cfg = cx.global::<crate::config::Config>().clone();
    let mut group = SettingGroup::new()
        .title("Built-in agents")
        .description("Arguments appended to each agent's command.");
    for (label, program) in crate::config::BUILTIN_AGENTS {
        let current = cfg.agent_args.get(*program).cloned().unwrap_or_default();
        let program_key = program.to_string();
        let set: std::rc::Rc<dyn Fn(String, &mut App)> = std::rc::Rc::new(move |value, cx| {
            let key = program_key.clone();
            update_config(
                move |c, _| {
                    c.agent_args.insert(key, value);
                },
                cx,
            );
        });
        // Custom element: brand-colored icon + name as the row header,
        // the args input below at full group width (all rows equal).
        group = group.item(
            SettingItem::render(move |options, window, cx| {
                let tint = super::agent_tint(program, cx);
                v_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                super::agent_icon(program)
                                    .size_3p5()
                                    .flex_none()
                                    .text_color(tint),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(cx.theme().foreground.opacity(0.9))
                                    .child(*label),
                            ),
                    )
                    .child(commit_input(
                        format!("builtin-args-{program}"),
                        current.clone(),
                        set.clone(),
                        options,
                        window,
                        cx,
                    ))
            })
            .keywords(["Args", *label, *program]),
        );
    }
    group
}

/// Custom agents: one bordered group per agent, reading as a unit
/// instead of a flat run of identical rows. Each group opens with one
/// header row (brand mark + name + a trailing × that asks for
/// confirmation), then stacked full-width Name / Program / Args inputs.
/// "Custom agents" itself is a plain group whose own header row is the
/// + affordance (icon + "Add agent", ghost, small) — visible even with
/// zero agents, since a title-only group would render no row at all.
pub(super) fn custom_agents_groups(cx: &App) -> Vec<SettingGroup> {
    let cfg = cx.global::<crate::config::Config>().clone();
    let mut groups = Vec::new();
    // No stock `.title()` here: the group needs a `+` button on the
    // right of its header row, which `SettingGroup::title` (a plain
    // SharedString) cannot host. Render the header inside the group,
    // first item: title text styled like GroupBox's default title,
    // plus button pinned right — visible even with zero agents.
    groups.push(SettingGroup::new().item(SettingItem::render({
        move |options, _, cx| {
            let muted = cx.theme().muted_foreground;
            v_flex()
                .w_full()
                .gap_1()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_color(muted)
                                .line_height(relative(1.))
                                .child("Custom agents"),
                        )
                        .child(
                            Button::new("add-agent")
                                .icon(IconName::Plus)
                                .ghost()
                                .xsmall()
                                .tab_stop(false)
                                .tooltip("Add agent")
                                .disabled(options.is_disabled())
                                .on_click(|_, _, cx| {
                                    update_config(
                                        |c, _| {
                                            let mut number = 1;
                                            while c
                                                .custom_agents
                                                .iter()
                                                .any(|a| a.name == format!("agent-{number}"))
                                            {
                                                number += 1;
                                            }
                                            c.custom_agents.push(crate::config::AgentPreset {
                                                name: format!("agent-{number}"),
                                                program: String::new(),
                                                args: String::new(),
                                            });
                                        },
                                        cx,
                                    );
                                }),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("Extra launchers for the sidebar menus."),
                )
        }
    })));
    for ix in 0..cfg.custom_agents.len() {
        let a = cfg.custom_agents[ix].clone();
        let command = if a.program.trim().is_empty() {
            "No program set yet".to_string()
        } else {
            format!("{} {}", a.program, a.args).trim_end().to_string()
        };
        let name = a.name.clone();
        let program = a.program.clone();
        let args = a.args.clone();
        let tint = super::agent_tint(&a.name, cx);
        let title: SharedString = if name.trim().is_empty() {
            "Untitled agent".into()
        } else {
            SharedString::from(name.clone())
        };
        let mut group = SettingGroup::new()
            .title(title)
            .description(command)
            // Header row FIRST (same offset as an input's label
            // column): brand mark + name, × pinned right — the name
            // still edits below, so this row is display-only.
            .item(
                SettingItem::render({
                    let name = name.clone();
                    let kind = a.name.clone();
                    move |options, _, _cx| {
                        let agent_name = name.clone();
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .child(
                                super::agent_icon(&kind)
                                    .size_3p5()
                                    .flex_none()
                                    .text_color(tint),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(name.clone()),
                            )
                            .child(
                                Button::new(("remove-agent", ix))
                                    .icon(IconName::Close)
                                    .danger()
                                    .ghost()
                                    .xsmall()
                                    .tab_stop(false)
                                    .tooltip(format!("Remove {name}"))
                                    .disabled(options.is_disabled())
                                    .on_click(move |_, window, cx| {
                                        let ix = ix;
                                        let agent_name = agent_name.clone();
                                        window.open_alert_dialog(cx, move |alert, _, _| {
                                            let ix = ix;
                                            let agent_name = agent_name.clone();
                                            // Same Cancel + danger-confirm
                                            // recipe as every other dialog —
                                            // see `ui::dialog_footer`.
                                            alert
                                                .title(format!("Remove “{agent_name}”?"))
                                                .description(
                                                    "The agent disappears from the sidebar menus.",
                                                )
                                                .footer(super::dialog_footer(
                                                    "Remove",
                                                    ("confirm-remove", ix),
                                                    move |_, window, cx| {
                                                        window.close_dialog(cx);
                                                        update_config(
                                                            move |c, _| {
                                                                if ix < c.custom_agents.len() {
                                                                    let removed =
                                                                        c.custom_agents.remove(ix);
                                                                    if c.new_session.kind
                                                                        == removed.name
                                                                    {
                                                                        c.new_session.kind =
                                                                            "terminal".into();
                                                                    }
                                                                }
                                                            },
                                                            cx,
                                                        );
                                                    },
                                                ))
                                        });
                                    }),
                            )
                    }
                })
                .keywords([
                    SharedString::from("Remove"),
                    SharedString::from(name.clone()),
                ]),
            );
        group = group
            // Stacked label-over-input rows: the inputs span the group
            // width instead of leaving dead space to their right.
            .item(
                SettingItem::new(
                    "Name",
                    commit_text_field(
                        move |_| name.clone(),
                        move |value, cx| {
                            let ix = ix;
                            update_config(
                                move |c, _| {
                                    if let Some(agent) = c.custom_agents.get_mut(ix) {
                                        if c.new_session.kind == agent.name {
                                            c.new_session.kind = value.clone();
                                        }
                                        agent.name = value;
                                    }
                                },
                                cx,
                            );
                        },
                    ),
                )
                .layout(Axis::Vertical),
            )
            .item(
                SettingItem::new(
                    "Program",
                    commit_text_field(
                        move |_| program.clone(),
                        move |value, cx| {
                            let ix = ix;
                            update_config(
                                move |c, _| {
                                    if let Some(agent) = c.custom_agents.get_mut(ix) {
                                        agent.program = value;
                                    }
                                },
                                cx,
                            );
                        },
                    ),
                )
                .layout(Axis::Vertical),
            )
            .item(
                SettingItem::new(
                    "Args",
                    commit_text_field(
                        move |_| args.clone(),
                        move |value, cx| {
                            let ix = ix;
                            update_config(
                                move |c, _| {
                                    if let Some(agent) = c.custom_agents.get_mut(ix) {
                                        agent.args = value;
                                    }
                                },
                                cx,
                            );
                        },
                    ),
                )
                .layout(Axis::Vertical),
            );
        groups.push(group);
    }
    groups
}
