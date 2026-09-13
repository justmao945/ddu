//! Theme appearance: the mode switch, the terminal font picker and
//! the terminal buffer (scrollback) settings.

use std::rc::Rc;

use super::update_config;
use crate::config::{TERMINAL_FONT_SIZE_DEFAULT, TERMINAL_SCROLLBACK_DEFAULT};
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{
    InputEvent, InputState, NumberInput, NumberInputEvent, StepAction,
};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::setting::{RenderOptions, SettingItem};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

pub(crate) fn set_theme(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
    // `Theme::change` re-applies the registry theme config; keep the
    // base size set at startup (see `main.rs`), and the configured
    // mono typography (the registry resets it to the stock 13px +
    // platform family).
    Theme::global_mut(cx).font_size = px(crate::config::ui_font_size());
    crate::ui::apply_mono_typography(cx);
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
pub(super) fn theme_item() -> SettingItem {
    super::item("Theme", "Color scheme for the interface.", |options, _, cx| {
        let dark = Theme::global(cx).is_dark();
        Button::new("theme-select")
            .accessibility_label(format!("Theme: {}", if dark { "Dark" } else { "Light" }))
            .label(if dark { "Dark" } else { "Light" })
            .dropdown_caret(true)
            .outline()
            .disabled(options.is_disabled())
            .with_size(options.size())
            .w(px(super::CONTROL_W))
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                menu.min_w(px(super::CONTROL_W))
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

/// Width of the font picker trigger; the popup menu matches it exactly
/// so the two read as one control.
pub(super) const FONT_PICKER_W: f32 = 300.;

/// Terminal font size clamp (px). Keeps a mistyped value from breaking
/// the grid metrics while still allowing any sane size.
const FONT_SIZE_MIN: f32 = 8.;
const FONT_SIZE_MAX: f32 = 32.;

/// ⌘+/⌘− zoom: step the terminal font size by `delta`, persist it and
/// apply it to the live theme (the same path the settings field uses).
pub(crate) fn bump_font_size(delta: f32, cx: &mut App) {
    let current = cx.global::<crate::config::Config>().terminal_font_size();
    let size = (current + delta).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
    Theme::global_mut(cx).mono_font_size = px(size);
    update_config(move |c, _| c.terminal_font_size = Some(size), cx);
}

/// State behind a [`number_spinner`] control: the input plus the value
/// it was last synced from, so external changes (another window, a ⌘±
/// zoom) can rewrite the field and typed edits only fire when the user
/// actually changed something. Mirrors the stock `NumberField`.
struct NumberState {
    input: Entity<InputState>,
    initial_value: f64,
    _subscriptions: Vec<Subscription>,
}

/// Spinner control with the stock `SettingField::number_input` behavior
/// (typed edits clamp to `[min, max]`, ▲▼ buttons step by `step`),
/// hosted by the custom item layout. `key` must be unique per settings
/// window — one spinner per [page + item] is enough in practice.
fn number_spinner(
    key: &'static str,
    options: &RenderOptions,
    window: &mut Window,
    cx: &mut App,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    set: impl Fn(f64, &mut App) + 'static,
) -> AnyElement {
    let set_value: Rc<dyn Fn(f64, &mut App)> = Rc::new(set);
    let step_set_value = set_value.clone();

    let state_entity = window.use_keyed_state(
        SharedString::from(format!("number-state-{key}")),
        cx,
        move |window, cx| {
            let input =
                cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));
            let _subscriptions = vec![
                cx.subscribe_in(&input, window, {
                    move |state: &mut NumberState, input, event: &NumberInputEvent, window, cx| {
                        match event {
                            NumberInputEvent::Step(action) => {
                                let value = input.read(cx).value();
                                if let Ok(value) = value.parse::<f64>() {
                                    let new_value = if *action == StepAction::Increment {
                                        value + step
                                    } else {
                                        value - step
                                    };
                                    let clamp_value = new_value.clamp(min, max);

                                    input.update(cx, |input, cx| {
                                        input.set_value(
                                            SharedString::from(clamp_value.to_string()),
                                            window,
                                            cx,
                                        );
                                    });
                                    step_set_value(clamp_value, cx);
                                    state.initial_value = clamp_value;
                                }
                            }
                        }
                    }
                }),
                cx.subscribe_in(&input, window, {
                    move |state: &mut NumberState, input, event: &InputEvent, window, cx| {
                        match event {
                            InputEvent::Change => {
                                input.update(cx, |input, cx| {
                                    let value = input.value();
                                    if value == state.initial_value.to_string() {
                                        return;
                                    }

                                    if let Ok(value) = value.parse::<f64>() {
                                        let clamp_value = value.clamp(min, max);

                                        set_value(clamp_value, cx);
                                        state.initial_value = clamp_value;
                                        if clamp_value != value {
                                            input.set_value(
                                                SharedString::from(clamp_value.to_string()),
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                });
                            }
                            _ => {}
                        }
                    }
                }),
            ];

            NumberState {
                input,
                initial_value: value,
                _subscriptions,
            }
        },
    );

    // Sync the displayed value when the underlying setting changed externally.
    state_entity.update(cx, |state, cx| {
        if state.initial_value != value {
            state.initial_value = value;
            state.input.update(cx, |input, cx| {
                input.set_value(SharedString::from(value.to_string()), window, cx);
            });
        }
    });

    let state = state_entity.read(cx);

    NumberInput::new(&state.input)
        .disabled(options.is_disabled())
        .with_size(options.size())
        .w_32()
        .into_any_element()
}

/// The zoom chord is platform-specific (⌘ on macOS, ⌃ elsewhere) and a
/// settings row wants a `&'static str` for its keyword index, so each
/// platform spells the sentence out.
#[cfg(target_os = "macos")]
const SIZE_DESCRIPTION: &str =
    "Mono font size in points for all terminal sessions, clamped to 8–32, \
     with ⌘+ and ⌘− zooming from anywhere.";
#[cfg(not(target_os = "macos"))]
const SIZE_DESCRIPTION: &str =
    "Mono font size in points for all terminal sessions, clamped to 8–32, \
     with Ctrl++ and Ctrl+− zooming from anywhere.";

/// Terminal font size (px): a spinner field — type a value, or step
/// with the ▲▼ buttons / arrow keys. Every change applies to the live
/// theme and persists immediately.
pub(super) fn terminal_size_item() -> SettingItem {
    super::item(
        "Size",
        SIZE_DESCRIPTION,
        |options, window, cx| {
            let size = cx.global::<crate::config::Config>().terminal_font_size() as f64;
            number_spinner(
                "size",
                options,
                window,
                cx,
                size,
                FONT_SIZE_MIN as f64,
                FONT_SIZE_MAX as f64,
                1.,
                |size, cx| {
                    let size = size as f32;
                    Theme::global_mut(cx).mono_font_size = px(size);
                    update_config(move |c, _| c.terminal_font_size = Some(size), cx);
                },
            )
        },
    )
    .on_reset(
        |cx| {
            cx.global::<crate::config::Config>()
                .terminal_font_size
                .is_some()
        },
        |_, cx| {
            Theme::global_mut(cx).mono_font_size =
                px(TERMINAL_FONT_SIZE_DEFAULT * crate::config::desktop_text_scale());
            update_config(|c, _| c.terminal_font_size = None, cx);
        },
    )
}

/// Terminal scrollback cap (lines): a spinner field. Applies to newly
/// spawned sessions — an existing grid keeps the cap it was built with.
pub(super) fn terminal_scrollback_item() -> SettingItem {
    super::item(
        "Lines",
        "Maximum scrollback history per session, with the oldest lines dropped \
         past the cap and the limit applied to newly spawned sessions.",
        |options, window, cx| {
            let lines = cx.global::<crate::config::Config>().terminal_scrollback() as f64;
            number_spinner(
                "scrollback",
                options,
                window,
                cx,
                lines,
                100.,
                100_000.,
                500.,
                |lines, cx| {
                    update_config(move |c, _| c.terminal_scrollback = Some(lines as usize), cx)
                },
            )
        },
    )
    .on_reset(
        |cx| {
            cx.global::<crate::config::Config>().terminal_scrollback()
                != TERMINAL_SCROLLBACK_DEFAULT
        },
        |_, cx| update_config(|c, _| c.terminal_scrollback = None, cx),
    )
}

pub(super) fn terminal_font_item() -> SettingItem {
    const SYSTEM_DEFAULT: &str = "__system_default__";
    super::item(
        "Font",
        "Typeface for all terminal sessions, falling back to the platform \
         monospace font when set to System default.",
        |options, window, cx| {
        let configured = cx
            .global::<crate::config::Config>()
            .terminal_font
            .clone()
            .unwrap_or_default();
        let mut families: Vec<String> = window.text_system().all_font_names();
        families.retain(|f| crate::ui::is_mono_family(f));
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
            .accessibility_label(format!("Terminal font: {label}"))
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
                            crate::ui::apply_mono_typography(cx);
                        }),
                    );
                }
                m
            })
    })
}

