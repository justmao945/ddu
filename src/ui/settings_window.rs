//! Settings window: the sidebar-based [`Settings`] surface in its own
//! native window, opened from the title-bar gear or ⌘,. Appearance
//! (theme) lives here; future config appends as new pages.
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::IconName;
use gpui_kit::component::Root;
use gpui_kit::component::Side;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// The one settings window, while open (singleton slot).
struct SettingsWindowSlot(Option<AnyWindowHandle>);
impl Global for SettingsWindowSlot {}

/// Root view of the standalone settings window. Config edits go through
/// [`update_config`], which persists and refreshes every window, so the
/// main workspace picks changes up live.
pub(crate) struct SettingsWindow {
    focus: FocusHandle,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(false);
        focus.focus(window, cx);
        Self { focus }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .track_focus(&self.focus)
            .key_context("SettingsWindow")
            .on_action(|_: &crate::app::CloseSettings, window, _| window.remove_window())
            .child(
                Settings::new("ddu-settings")
                    // One compact control size for every field, so the
                    // custom buttons/dropdowns below match the stock ones.
                    .with_size(gpui_kit::component::Size::Small)
                    // Dev hook (DDU_VERIFY_SETTINGS=<page_ix>): open on a
                    // specific page for screenshot verification — synthetic
                    // clicks aren't available in the harness environment.
                    .when_some(
                        std::env::var("DDU_VERIFY_SETTINGS").ok().and_then(|v| v.parse::<usize>().ok()),
                        |this, ix| {
                            this.default_selected_index(
                                gpui_kit::component::setting::SelectIndex { page_ix: ix, group_ix: None },
                            )
                        },
                    )
                    .with_group_variant(GroupBoxVariant::Outline)
                    .page(
                        SettingPage::new("Appearance")
                            .header_style(&page_header_style())
                            .icon(IconName::Palette)
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
                                    .title("Shell")
                                    .item(
                                        SettingItem::new("Program", shell_program_field())
                                            .layout(Axis::Vertical)
                                            .description("Program for Terminal sessions."),
                                    )
                                    .item(
                                        SettingItem::new("Args", shell_args_field())
                                            .layout(Axis::Vertical)
                                            .description("Arguments passed to the shell."),
                                    ),
                            )
                            .group(
                                SettingGroup::new().title("Font").item(
                                    SettingItem::new("Font", terminal_font_field())
                                        .layout(Axis::Vertical)
                                        .description(
                                            "Typeface for all sessions. System default uses the platform monospace font.",
                                        ),
                                ),
                            )
                    )
                    .page(
                        SettingPage::new("Sessions")
                            .header_style(&page_header_style())
                            .icon(IconName::Bot)
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
            // Overlay layers (anchored, no layout impact): dropdown
            // menus and any dialog/notification a component raises are
            // hosted here, same as the main window root.
            .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
            .children(gpui_kit::component::Root::render_notification_layer(
                window, cx,
            ))
    }
}

/// Title-bar gear button that opens the settings window.
pub(crate) fn button() -> impl IntoElement {
    Button::new("open-settings")
        .icon(IconName::Settings)
        .ghost()
        .small()
        .tab_stop(false)
        .tooltip("Settings (⌘,)")
        .on_click(|_, _, cx| open(cx))
}

/// Open the settings window, or bring the existing one forward — the
/// gear and ⌘, never spawn a duplicate.
pub(crate) fn open(cx: &mut App) {
    let existing = cx
        .try_global::<SettingsWindowSlot>()
        .and_then(|slot| slot.0);
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    // Centered on the primary display (gpui has no parent-relative
    // centering for `open_window`): a fixed 880×600 at a point that
    // puts it near-center on common laptop/desktop sizes without
    // hiding the workspace behind it.
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(880.), px(600.)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        app_owns_titlebar_drag: false,
        inactive_frame_interval: None,
        is_resizable: true,
        is_minimizable: true,
        display_id: None,
        window_background: WindowBackgroundAppearance::Opaque,
        app_id: None,
        window_min_size: Some(size(px(640.), px(440.))),
        window_decorations: None,
        icon: None,
        tabbing_identifier: None,
    };
    match cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| SettingsWindow::new(window, cx));
        // First level on the window must be a Root — `*_dialog`,
        // notification and menu overlays all `expect` it.
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        Ok(handle) => {
            if cx.try_global::<SettingsWindowSlot>().is_none() {
                cx.set_global(SettingsWindowSlot(None));
            }
            cx.global_mut::<SettingsWindowSlot>().0 = Some(handle.into());
        }
        Err(err) => eprintln!("failed to open settings window: {err}"),
    }
}

/// Update the global config and persist from a settings field.
/// No-ops (e.g. a blur commit with an unchanged value) skip the disk
/// write and the window refresh.
fn update_config(f: impl FnOnce(&mut crate::config::Config, &mut App), cx: &mut App) {
    let before = cx.global::<crate::config::Config>().clone();
    cx.update_global::<crate::config::Config, _>(f);
    if *cx.global::<crate::config::Config>() == before {
        return;
    }
    let snapshot = cx.global::<crate::config::Config>().clone();
    snapshot.save();
    cx.refresh_windows();
}

/// State for [`commit_text_field`].
struct CommitFieldState {
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// The composable half of [`commit_text_field`]: an [`Input`] that
/// commits on Enter or blur, keyed by `key`, resyncing from `current`
/// whenever the field isn't focused. Returns the bare component so
/// callers can embed it in custom rows (e.g. the builtin-agent line).
fn commit_input(
    key: String,
    current: String,
    set: std::rc::Rc<dyn Fn(String, &mut App)>,
    options: &gpui_kit::component::setting::RenderOptions,
    window: &mut Window,
    cx: &mut App,
) -> Input {
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
            CommitFieldState {
                input,
                _subscription: subscription,
            }
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
        let key = format!(
            "commit-input-{}-{}-{}",
            options.page_ix(),
            options.group_ix(),
            options.item_ix()
        );
        commit_input(key, get(cx), set.clone(), options, window, cx).map(|this| {
            if matches!(options.layout(), Axis::Horizontal) {
                this.w_64()
            } else {
                this.w_full()
            }
        })
    })
}
/// Default new-session type picker: Terminal / builtins / custom agents.
fn default_session_field() -> SettingField<SharedString> {
    SettingField::<SharedString>::render(|options, _, cx| {
        let cfg = cx.global::<crate::config::Config>().clone();
        let current = cfg.new_session.kind.clone();
        Button::new("default-session-select")
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        super::agent_icon(&current)
                            .xsmall()
                            .flex_none()
                            .text_color(super::agent_tint(&current, cx)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(cfg.label_for(&current)),
                    ),
            )
            .dropdown_caret(true)
            .outline()
            .disabled(options.is_disabled())
            .with_size(options.size())
            .w(px(170.))
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let mut m = menu.min_w(px(170.)).check_side(Side::Right);
                for kind in cfg.agent_menu() {
                    let label = cfg.label_for(&kind);
                    let checked = kind == current;
                    let k = kind.clone();
                    let k_row = kind.clone();
                    m = m.item(
                        PopupMenuItem::element(move |_, cx| {
                            super::agent_menu_row(&k_row, label.clone(), cx)
                        })
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
/// Common macOS shells for the program dropdown. Only entries that
/// actually exist on this machine are listed — uninstalled shells
/// would spawn-and-die with a cryptic PTY error.
const SHELLS: &[&str] = &[
    "/bin/zsh",
    "/bin/bash",
    "/bin/sh",
    "/opt/homebrew/bin/fish",
    "/usr/local/bin/fish",
    "/opt/homebrew/bin/nu",
    "/usr/local/bin/nu",
];

fn installed_shells() -> Vec<&'static str> {
    SHELLS
        .iter()
        .copied()
        .filter(|s| std::path::Path::new(s).is_file())
        .collect()
}

/// Login shell picker: a fixed dropdown (there are only a handful of
/// shells), keeping the trigger and its menu one aligned control.
fn shell_program_field() -> SettingField<SharedString> {
    SettingField::<SharedString>::render(|options, _, cx| {
        let cfg = cx.global::<crate::config::Config>().clone();
        let current = cfg.shell.program.clone();
        Button::new("shell-program-select")
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(current.clone()),
            )
            .dropdown_caret(true)
            .outline()
            .disabled(options.is_disabled())
            .with_size(options.size())
            .w(px(220.))
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                // Always keep the current value selectable (it may be
                // a custom path typed before), plus installed shells.
                let mut shells = vec![current.as_str()];
                shells.extend(installed_shells());
                shells.sort();
                shells.dedup();
                let mut m = menu.min_w(px(220.)).check_side(Side::Right);
                for shell in shells {
                    let checked = shell == current;
                    let s = shell.to_string();
                    m = m.item(PopupMenuItem::new(shell).checked(checked).on_click(
                        move |_, _, cx| {
                            let s = s.clone();
                            update_config(|c, _| c.shell.program = s, cx);
                        },
                    ));
                }
                m
            })
    })
}

fn shell_args_field() -> SettingField<SharedString> {
    commit_text_field(
        |cx| cx.global::<crate::config::Config>().shell.args.clone(),
        |value, cx| {
            update_config(|c, _| c.shell.args = value, cx);
        },
    )
}

/// Width of the font picker trigger; the popup menu matches it exactly
/// so the two read as one control.
const FONT_PICKER_W: f32 = 300.;

fn terminal_font_field() -> SettingField<SharedString> {
    const SYSTEM_DEFAULT: &str = "__system_default__";
    SettingField::<SharedString>::render(move |options, window, cx| {
        let configured = cx
            .global::<crate::config::Config>()
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
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(label),
            )
            .dropdown_caret(true)
            .outline()
            .disabled(options.is_disabled())
            .with_size(options.size())
            .w(px(FONT_PICKER_W))
            .min_w_0()
            .max_w_full()
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let mut m = menu
                    .min_w(px(FONT_PICKER_W))
                    .max_w(px(FONT_PICKER_W))
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
                    // Each family name set in its own face, so the list
                    // doubles as a specimen sheet.
                    let specimen = family.clone();
                    m = m.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_family(specimen.clone())
                                .child(specimen.clone())
                        })
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            let family = family.clone();
                            update_config(move |c, _| c.terminal_font = Some(family.clone()), cx);
                        }),
                    );
                }
                m
            })
    })
}

/// Monospace detection. Font registries expose no `is_monospace` flag,
/// so match on the family name: anything saying "mono" (minus the
/// proportional "propo" Nerd Font variants), plus a substring list of
/// known mono families whose names don't say it.
fn is_mono_family(family: &str) -> bool {
    const MONO_NAME_HINTS: &[&str] = &[
        "menlo",
        "monaco",
        "courier",
        "consolas",
        "meslo",
        "fira code",
        "source code pro",
        "cascadia code",
        "hack",
        "inconsolata",
        "iosevka",
        "monaspace",
        "sarasa term",
    ];
    let f = family.to_ascii_lowercase();
    if f.contains("propo") {
        return false;
    }
    f.contains("mono") || MONO_NAME_HINTS.iter().any(|h| f.contains(h))
}

fn builtin_agent_groups(cx: &App) -> SettingGroup {
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
fn custom_agents_groups(cx: &App) -> Vec<SettingGroup> {
    let cfg = cx.global::<crate::config::Config>().clone();
    let mut groups = Vec::new();
    groups.push(
        SettingGroup::new()
            .title("Custom agents")
            .description("Extra launchers for the sidebar menus.")
            .item(SettingItem::render(|options, _, _| {
                Button::new("add-agent")
                    .icon(IconName::Plus)
                    .label("Add agent")
                    .ghost()
                    .small()
                    .tab_stop(false)
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
                    })
            })),
    );
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

/// Zed-style page header: a prominent 16px medium title above the muted
/// group titles, without the stock header's bottom hairline.
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
    // `Theme::change(.., None, ..)` refreshes no window — repaint all,
    // or existing terminals/panels keep the old palette until their
    // next wakeup.
    cx.refresh_windows();
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
