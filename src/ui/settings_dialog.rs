//! Settings dialog: classic sidebar-based [`Settings`] surface inside a
//! [`Dialog`], opened from the title-bar gear. Appearance (theme) lives here;
//! future config (fonts, agent commands, persistence) appends as new pages.
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{IconName, WindowExt as _};
use gpui_kit::*;

/// Title-bar gear button that opens the settings dialog.
pub(crate) fn button() -> impl IntoElement {
    Button::new("open-settings")
        .icon(IconName::Settings)
        .ghost()
        .small()
        .tooltip("Settings")
        .on_click(|_, window, cx| open(window, cx))
}

pub(crate) fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, cx| {
        dialog
            .title(
                div()
                    .pt_3()
                    .px_4()
                    .pb_3()
                    .mb(-px(8.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .font_medium()
                    .child("Settings"),
            )
            .overflow_hidden()
            .p_0()
            .w(px(840.))
            .content(|content, _, cx| {
                content.h(px(560.)).text_sm().child(
                    Settings::new("ddu-settings")
                        .with_group_variant(GroupBoxVariant::Outline)
                        .sidebar_style(&sidebar_corner_style(cx.theme().radius_lg))
                        .page(
                            SettingPage::new("Appearance")
                                .header_style(&page_header_style())
                                .icon(IconName::Settings)
                                .group(
                                    SettingGroup::new().item(
                                        SettingItem::new("Theme", theme_field())
                                            .description("Color scheme for the interface."),
                                    ),
                                ),
                        )
                        .page(
                            SettingPage::new("Sessions")
                                .header_style(&page_header_style())
                                .icon(IconName::SquareTerminal)
                                .group(
                                    SettingGroup::new()
                                        .title("New session")
                                        .item(
                                            SettingItem::new(
                                                "Default type",
                                                default_session_field(),
                                            )
                                            .description(
                                                "What the sidebar + button creates.",
                                            ),
                                        ),
                                )
                                .group(
                                    SettingGroup::new()
                                        .title("Terminal")
                                        .item(
                                            SettingItem::new("Shell", shell_program_field())
                                                .description("Program for Terminal sessions."),
                                        )
                                        .item(
                                            SettingItem::new("Shell args", shell_args_field())
                                                .description("Extra arguments (space separated)."),
                                        )
                                        .item(
                                            SettingItem::new("Font", terminal_font_field())
                                                .description(
                                                    "Family name; empty = system mono.",
                                                ),
                                        ),
                                )
                                .group(builtin_agent_groups(cx))
                                .group(custom_agents_group(cx)),
                        ),
                )
            })
    })
}

/// Update the global config and persist from a settings field.
fn update_config(f: impl FnOnce(&mut crate::config::Config, &mut App), cx: &mut App) {
    cx.update_global::<crate::config::Config, _>(f);
    let snapshot = cx.global::<crate::config::Config>().clone();
    snapshot.save();
    cx.refresh_windows();
}

/// Default new-session type picker: Terminal / builtins / custom agents.
fn default_session_field() -> SettingField<SharedString> {
    SettingField::<SharedString>::render(|_, _, cx| {
        let cfg = cx.global::<crate::config::Config>().clone();
        let current = cfg.new_session.kind.clone();
        Button::new("default-session-select")
            .label(cfg.label_for(&current))
            .dropdown_caret(true)
            .outline()
            .w(px(150.))
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let mut m = menu;
                for kind in cfg.agent_menu() {
                    let label = cfg.label_for(&kind);
                    let checked = kind == current;
                    let k = kind.clone();
                    m = m.item(
                        PopupMenuItem::new(label)
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                let k = k.clone();
                                update_config(|c, _| c.new_session.kind = k, cx);
                            }),
                    );
                }
                m
            })
    })
}

fn shell_program_field() -> SettingField<SharedString> {
    SettingField::<SharedString>::input(
        |cx| {
            cx.global::<crate::config::Config>()
                .shell
                .program
                .clone()
                .into()
        },
        |value, cx| {
            let v = value.to_string();
            update_config(|c, _| c.shell.program = v, cx);
        },
    )
}

fn shell_args_field() -> SettingField<SharedString> {
    SettingField::<SharedString>::input(
        |cx| {
            cx.global::<crate::config::Config>()
                .shell
                .args
                .clone()
                .into()
        },
        |value, cx| {
            let v = value.to_string();
            update_config(|c, _| c.shell.args = v, cx);
        },
    )
}

/// Terminal font family input; empty means the system default mono face.
fn terminal_font_field() -> SettingField<SharedString> {
    SettingField::<SharedString>::input(
        |cx| {
            cx.global::<crate::config::Config>()
                .terminal_font
                .clone()
                .unwrap_or_default()
                .into()
        },
        |value, cx| {
            let v = value.trim().to_string();
            update_config(
                move |c, _| {
                    c.terminal_font = (!v.is_empty()).then_some(v.clone());
                },
                cx,
            );
        },
    )
}

/// One arg-input item per builtin agent.
fn builtin_agent_groups(cx: &App) -> SettingGroup {
    let cfg = cx.global::<crate::config::Config>().clone();
    let mut group = SettingGroup::new().title("Built-in agents");
    for (label, program) in crate::config::BUILTIN_AGENTS {
        let program_key = program.to_string();
        let current = cfg.agent_args.get(*program).cloned().unwrap_or_default();
        group = group.item(
            SettingItem::new(
                *label,
                SettingField::<SharedString>::input(
                    move |_| current.clone().into(),
                    move |value, cx| {
                        let v = value.to_string();
                        let key = program_key.clone();
                        update_config(move |c, _| { c.agent_args.insert(key, v); }, cx);
                    },
                ),
            )
            .description(format!("`{program} <args>`")),
        );
    }
    group
}

/// Custom agents: one row (name + program + args) per entry, plus an
/// add button. Editing writes through to the config immediately.
fn custom_agents_group(cx: &App) -> SettingGroup {
    let cfg = cx.global::<crate::config::Config>().clone();
    let mut group = SettingGroup::new().title("Custom agents");
    for ix in 0..cfg.custom_agents.len() {
        let a = cfg.custom_agents[ix].clone();
        let name = a.name.clone();
        let program = a.program.clone();
        let args = a.args.clone();
        group = group
            .item(
                SettingItem::new(
                    "Name",
                    SettingField::<SharedString>::input(
                        move |_| name.clone().into(),
                        move |value, cx| {
                            let v = value.to_string();
                            let ix = ix;
                            update_config(move |c, _| c.custom_agents[ix].name = v, cx);
                        },
                    ),
                ),
            )
            .item(
                SettingItem::new(
                    "Program",
                    SettingField::<SharedString>::input(
                        move |_| program.clone().into(),
                        move |value, cx| {
                            let v = value.to_string();
                            let ix = ix;
                            update_config(move |c, _| c.custom_agents[ix].program = v, cx);
                        },
                    ),
                ),
            )
            .item(
                SettingItem::new(
                    "Args",
                    SettingField::<SharedString>::input(
                        move |_| args.clone().into(),
                        move |value, cx| {
                            let v = value.to_string();
                            let ix = ix;
                            update_config(move |c, _| c.custom_agents[ix].args = v, cx);
                        },
                    ),
                ),
            )
            .item(
                SettingItem::new("Remove", SettingField::<SharedString>::render(move |_, _, _cx| {
                    let ix = ix;
                    Button::new(("remove-agent", ix))
                        .label("Remove agent")
                        .outline()
                        .small()
                        .on_click(move |_, _, cx| {
                            update_config(move |c, _| {
                                if ix < c.custom_agents.len() {
                                    c.custom_agents.remove(ix);
                                }
                            }, cx);
                        })
                })),
            );
    }
    group.item(SettingItem::new(
        "Add",
        SettingField::<SharedString>::render(|_, _, _cx| {
            Button::new("add-agent")
                .label("Add custom agent")
                .outline()
                .small()
                .on_click(|_, _, cx| {
                    update_config(
                        |c, _| {
                            c.custom_agents.push(crate::config::AgentPreset {
                                name: format!("agent-{}", c.custom_agents.len() + 1),
                                program: String::new(),
                                args: String::new(),
                            });
                        },
                        cx,
                    );
                })
        }),
    ))
}

/// gpui clips children to the dialog's rectangular bounds, not its rounded
/// corners — the sidebar's fill must round its own bottom-left corner to
/// match the popup, or a square sliver pokes out past the curve.
fn sidebar_corner_style(radius: Pixels) -> StyleRefinement {
    let mut style = StyleRefinement::default();
    style.corner_radii.bottom_left = Some(AbsoluteLength::from(radius));
    style
}

/// Zed-style page header: a prominent 16px medium title above the muted
/// group titles. Also strips the stock header's bottom hairline — with the
/// dialog title's rule above, two stacked lines read heavy.
fn page_header_style() -> StyleRefinement {
    let mut style = StyleRefinement::default();
    style.text.font_size = Some(AbsoluteLength::from(px(16.)));
    style.text.font_weight = Some(FontWeight::MEDIUM);
    style.border_widths.bottom = Some(px(0.).into());
    style
}

/// Apply a theme mode globally and refresh every window.
pub(crate) fn set_theme(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
    cx.refresh_windows();
}

/// Theme select bound to the global [`Theme`] (light / dark).
///
/// A custom control (instead of `SettingField::dropdown`) so the popup menu
/// matches the trigger's fixed width — the stock dropdown's menu hugs its
/// label and looks misaligned against the trigger.
fn theme_field() -> SettingField<SharedString> {
    SettingField::<SharedString>::render(|options, _, cx| {
        let dark = Theme::global(cx).is_dark();
        Button::new("theme-select")
            .label(if dark { "Dark" } else { "Light" })
            .dropdown_caret(true)
            .outline()
            .disabled(options.is_disabled())
            .with_size(options.size())
            .w(px(150.))
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                menu.min_w(px(150.))
                    .item(
                        PopupMenuItem::new("Light")
                            .checked(!dark)
                            .on_click(|_, _, cx| set_theme(ThemeMode::Light, cx)),
                    )
                    .item(
                        PopupMenuItem::new("Dark")
                            .checked(dark)
                            .on_click(|_, _, cx| set_theme(ThemeMode::Dark, cx)),
                    )
            })
    })
}
