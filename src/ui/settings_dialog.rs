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
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{IconName, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Title-bar gear button that opens the settings dialog.
pub(crate) fn button() -> impl IntoElement {
    Button::new("open-settings")
        .icon(IconName::Settings)
        .ghost()
        .small()
        .tab_stop(false)
        .tooltip("Settings (⌘,)")
        .on_click(|_, window, cx| open(window, cx))
}

pub(crate) fn open(window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) { return; }
    window.open_dialog(cx, |dialog, window, cx| {
        let viewport = window.viewport_size();
        dialog
            .close_button(false)
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
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("Settings")
                    .child(Button::new("settings-close").small().ghost().icon(IconName::Close)
                        .on_click(|_, window, cx| window.close_dialog(cx))),
            )
            .overflow_hidden()
            .p_0()
            .w(px(840.).min(viewport.width - px(48.)))
            .content(move |content, _, cx| {
                content.h(px(560.).min((viewport.height - px(144.)).max(px(160.)))).text_sm().child(
                    div()
                        .size_full()
                        // gpui-component single-line Inputs propagate Enter
                        // on purpose; the Dialog context then maps it to
                        // Confirm, closing the dialog mid-edit. The
                        // DialogEnterSink binding (app.rs) outranks Confirm
                        // and this listener stops the dispatch loop, so
                        // Enter commits the field without dismissing.
                        .on_action(|_: &crate::app::DialogEnterSink, _, _| {})
                        .child(
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
                            SettingPage::new("Terminal")
                                .header_style(&page_header_style())
                                .icon(IconName::SquareTerminal)
                                .group(
                                    SettingGroup::new()
                                        .item(
                                            SettingItem::new("Shell", shell_program_field())
                                                .layout(Axis::Vertical)
                                                .description("Program for Terminal sessions."),
                                        )
                                        .item(
                                            SettingItem::new("Shell args", shell_args_field())
                                                .layout(Axis::Vertical)
                                                .description("Arguments; quote values containing spaces."),
                                        )
                                        .item(
                                            SettingItem::new("Font", terminal_font_field())
                                                .layout(Axis::Vertical)
                                                .description(
                                                    "Typeface for all sessions. System default uses the platform monospace font.",
                                                ),
                                        ),
                                ),
                        )
                        .page(
                            SettingPage::new("Sessions")
                                .header_style(&page_header_style())
                                .icon(IconName::PanelLeftOpen)
                                .group(
                                    SettingGroup::new().item(
                                        SettingItem::new(
                                            "Default type",
                                            default_session_field(),
                                        )
                                        .description(
                                            "What the sidebar + button creates.",
                                        ),
                                    ),
                                )
                                .group(builtin_agent_groups(cx))
                                .groups(custom_agents_groups(cx)),
                        ),
                    )
                )
            })
    })
}

/// Update the global config and persist from a settings field.
/// No-ops (e.g. a blur commit with an unchanged value) skip the disk
/// write and the window refresh.
fn update_config(f: impl FnOnce(&mut crate::config::Config, &mut App), cx: &mut App) {
    let before = cx.global::<crate::config::Config>().clone();
    cx.update_global::<crate::config::Config, _>(f);
    if *cx.global::<crate::config::Config>() == before { return; }
    let snapshot = cx.global::<crate::config::Config>().clone();
    snapshot.save();
    cx.refresh_windows();
}

/// State for [`commit_text_field`].
struct CommitFieldState {
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// Text field that commits on Enter or blur — not per keystroke, so a
/// half-typed value never lands in the config file and typing doesn't
/// trigger a save + full-window refresh per character.
fn commit_text_field(
    get: impl Fn(&App) -> String + 'static,
    set: impl Fn(String, &mut App) + 'static,
) -> SettingField<SharedString> {
    let set = std::rc::Rc::new(set);
    SettingField::<SharedString>::render(move |options, window, cx| {
        let current = get(cx);
        let key = format!(
            "commit-input-{}-{}-{}",
            options.page_ix(),
            options.group_ix(),
            options.item_ix()
        );
        let state = window.use_keyed_state(SharedString::from(key), cx, {
            let current = current.clone();
            let set = set.clone();
            move |window, cx| {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
                let subscription = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        set(input.read(cx).value().to_string(), cx);
                    }
                });
                CommitFieldState { input, _subscription: subscription }
            }
        });
        // Resync when the config changed from elsewhere — but never
        // clobber the text while the user is editing this field.
        state.update(cx, |state, cx| {
            let input = state.input.read(cx);
            let focused = input.focus_handle(cx).is_focused(window);
            if !focused && input.value() != current {
                state.input.update(cx, |input, cx| {
                    input.set_value(current.clone(), window, cx);
                });
            }
        });
        Input::new(&state.read(cx).input)
            .disabled(options.is_disabled())
            .with_size(options.size())
            .map(|this| {
                if matches!(options.layout(), Axis::Horizontal) { this.w_64() } else { this.w_full() }
            })
    })
}

/// Default new-session type picker: Terminal / builtins / custom agents.
fn default_session_field() -> SettingField<SharedString> {
    SettingField::<SharedString>::render(|_, _, cx| {
        let cfg = cx.global::<crate::config::Config>().clone();
        let current = cfg.new_session.kind.clone();
        Button::new("default-session-select")
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(cfg.label_for(&current)))
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
    commit_text_field(
        |cx| cx.global::<crate::config::Config>().shell.program.clone(),
        |value, cx| {
            update_config(|c, _| c.shell.program = value, cx);
        },
    )
}

fn shell_args_field() -> SettingField<SharedString> {
    commit_text_field(
        |cx| cx.global::<crate::config::Config>().shell.args.clone(),
        |value, cx| {
            update_config(|c, _| c.shell.args = value, cx);
        },
    )
}

/// Terminal font picker backed by the platform's installed faces.
///
/// A dropdown (not a text input) per the system-component convention: the
/// option list is the text system's font registry, headed by an explicit
/// `System default` entry that maps to the empty config value.
fn terminal_font_field() -> SettingField<SharedString> {
    const SYSTEM_DEFAULT: &str = "__system_default__";
    SettingField::<SharedString>::render(move |options, window, cx| {
        let configured = cx.global::<crate::config::Config>()
            .terminal_font
            .clone()
            .unwrap_or_default();
        let mut families: Vec<String> = window.text_system().all_font_names();
        families.retain(|f| is_mono_family(f));
        families.sort();
        families.dedup();
        let current: SharedString = if configured.is_empty() {
            SYSTEM_DEFAULT.into()
        } else {
            configured.clone().into()
        };
        let label = if configured.is_empty() {
            "System default".to_string()
        } else {
            configured
        };
        Button::new("terminal-font-select")
            .child(div().min_w_0().flex_1().overflow_hidden().text_ellipsis().child(label))
            .dropdown_caret(true)
            .outline()
            .disabled(options.is_disabled())
            .with_size(options.size())
            .w(px(220.))
            .min_w_0()
            .max_w_full()
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let mut m = menu
                    .min_w(px(220.))
                    .max_w(px(360.))
                    .max_h(px(420.))
                    .scrollable(true)
                    .item(
                        PopupMenuItem::new("System default")
                            .checked(current == SYSTEM_DEFAULT)
                            .on_click(|_, _, cx| {
                                update_config(|c, _| c.terminal_font = None, cx);
                            }),
                    );
                for family in &families {
                    let checked = current == family.as_str();
                    let family = family.clone();
                    m = m.item(
                        PopupMenuItem::new(family.clone())
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                let family = family.clone();
                                update_config(
                                    move |c, _| c.terminal_font = Some(family.clone()),
                                    cx,
                                );
                            }),
                    );
                }
                m
            })
    })
}

/// Monospace detection. gpui's text system exposes no `is_monospace`
/// flag, so gate the picker on the known mono families a terminal would
/// care about, intersected with what the platform actually has
/// installed (the caller retains over `all_font_names()`).
fn is_mono_family(family: &str) -> bool {
    const MONO_FAMILIES: &[&str] = &[
        "Menlo",
        "Monaco",
        "SF Mono",
        "SFMono-Regular",
        "Courier",
        "Courier New",
        "Courier Prime",
        "Andale Mono",
        "PT Mono",
        "Ubuntu Mono",
        "Ubuntu Sans Mono",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Sans Mono",
        "JetBrains Mono",
        "Fira Code",
        "Fira Mono",
        "Source Code Pro",
        "IBM Plex Mono",
        "Space Mono",
        "Roboto Mono",
        "Cascadia Code",
        "Cascadia Mono",
        "JetBrains Mono",
        "JetBrainsMono Nerd Font",
        "JetBrainsMono Nerd Font Mono",
        "JetBrains Mono NL",
        "Hack",
        "Inconsolata",
        "Iosevka",
        "Victor Mono",
        "Monaspace Neon",
        "Monaspace Argon",
        "Monaspace Xenon",
        "Monaspace Radon",
        "Monaspace Krypton",
        "Sarasa Mono SC",
        "Sarasa Term SC",
        "Maple Mono",
        "Maple Mono NF",
        "Maple Mono NF CN",
    ];
    MONO_FAMILIES
        .iter()
        .any(|m| family.eq_ignore_ascii_case(m))
}

/// One arg-input item per builtin agent.
fn builtin_agent_groups(cx: &App) -> SettingGroup {
    let cfg = cx.global::<crate::config::Config>().clone();
    let mut group = SettingGroup::new().title("Built-in agents")
        .description("Arguments appended to each agent's command.");
    for (label, program) in crate::config::BUILTIN_AGENTS {
        let program_key = program.to_string();
        let current = cfg.agent_args.get(*program).cloned().unwrap_or_default();
        group = group.item(
            SettingItem::new(
                *label,
                commit_text_field(
                    move |_| current.clone(),
                    move |value, cx| {
                        let key = program_key.clone();
                        update_config(move |c, _| { c.agent_args.insert(key, value); }, cx);
                    },
                ),
            )
            .description(format!("`{program} <args>`")),
        );
    }
    group
}

/// Custom agents: the Add button group first, then one bordered group
/// per agent (name as title, effective command as description) so each
/// agent reads as a unit instead of a flat run of identical rows.
fn custom_agents_groups(cx: &App) -> Vec<SettingGroup> {
    let cfg = cx.global::<crate::config::Config>().clone();
    let mut groups = Vec::new();
    groups.push(
        SettingGroup::new()
            .title("Custom agents")
            .description("Extra launchers for the sidebar menus. Values apply on Enter or when the field loses focus.")
            .item(SettingItem::new(
                "Add",
                SettingField::<SharedString>::render(|_, _, _cx| {
                    Button::new("add-agent")
                        .label("Add custom agent")
                        .outline()
                        .small()
                        .on_click(|_, _, cx| {
                            update_config(
                                |c, _| {
                                    let mut number = 1;
                                    while c.custom_agents.iter().any(|a| a.name == format!("agent-{number}")) { number += 1; }
                                    c.custom_agents.push(crate::config::AgentPreset {
                                        name: format!("agent-{number}"),
                                        program: String::new(),
                                        args: String::new(),
                                    });
                                },
                                cx,
                            );
                        })
                }),
            )),
    );
    for ix in 0..cfg.custom_agents.len() {
        let a = cfg.custom_agents[ix].clone();
        let command = if a.program.trim().is_empty() {
            "No program set yet".to_string()
        } else {
            format!("`{} {}`", a.program, a.args)
        };
        let name = a.name.clone();
        let program = a.program.clone();
        let args = a.args.clone();
        let group = SettingGroup::new()
            .title(a.name.clone())
            .description(command)
            .item(
                SettingItem::new(
                    "Name",
                    commit_text_field(
                        move |_| name.clone(),
                        move |value, cx| {
                            let ix = ix;
                            update_config(move |c, _| {
                                if let Some(agent) = c.custom_agents.get_mut(ix) {
                                    if c.new_session.kind == agent.name { c.new_session.kind = value.clone(); }
                                    agent.name = value;
                                }
                            }, cx);
                        },
                    ),
                ),
            )
            .item(
                SettingItem::new(
                    "Program",
                    commit_text_field(
                        move |_| program.clone(),
                        move |value, cx| {
                            let ix = ix;
                            update_config(move |c, _| { if let Some(agent) = c.custom_agents.get_mut(ix) { agent.program = value; } }, cx);
                        },
                    ),
                ),
            )
            .item(
                SettingItem::new(
                    "Args",
                    commit_text_field(
                        move |_| args.clone(),
                        move |value, cx| {
                            let ix = ix;
                            update_config(move |c, _| { if let Some(agent) = c.custom_agents.get_mut(ix) { agent.args = value; } }, cx);
                        },
                    ),
                ),
            )
            .item(
                SettingItem::new("Remove", SettingField::<SharedString>::render(move |_, _, _cx| {
                    let ix = ix;
                    Button::new(("remove-agent", ix))
                        .label("Remove this agent")
                        .outline()
                        .small()
                        .on_click(move |_, _, cx| {
                            update_config(move |c, _| {
                                if ix < c.custom_agents.len() {
                                    let removed = c.custom_agents.remove(ix);
                                    if c.new_session.kind == removed.name { c.new_session.kind = "terminal".into(); }
                                }
                            }, cx);
                        })
                }))
                .description("Delete this agent from the menus."),
            );
        groups.push(group);
    }
    groups
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

pub(crate) fn set_theme(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
    // `Theme::change` re-applies the registry theme config; keep the
    // compact 14px base set at startup (see `main.rs`).
    Theme::global_mut(cx).font_size = px(14.);
    update_config(|config, _| config.dark_theme = mode == ThemeMode::Dark, cx);
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
