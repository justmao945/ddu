//! Theme appearance: the mode switch, the terminal font picker and
//! the terminal buffer (scrollback) settings.

use super::update_config;
use crate::config::{TERMINAL_FONT_SIZE_DEFAULT, TERMINAL_SCROLLBACK_DEFAULT};
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::setting::{NumberFieldOptions, SettingField};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

pub(crate) fn set_theme(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
    // `Theme::change` re-applies the registry theme config; keep the
    // compact 14px base set at startup (see `main.rs`), and the
    // configured terminal mono size (the registry resets it to 13px).
    Theme::global_mut(cx).font_size = px(14.);
    Theme::global_mut(cx).mono_font_size =
        px(cx.global::<crate::config::Config>().terminal_font_size());
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
pub(super) fn theme_field() -> SettingField<SharedString> {
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

/// Terminal font size (px): a spinner field — type a value, or step
/// with the ▲▼ buttons / arrow keys. Every change applies to the live
/// theme and persists immediately.
pub(super) fn terminal_size_field() -> SettingField<f64> {
    SettingField::<f64>::number_input(
        NumberFieldOptions {
            min: FONT_SIZE_MIN as f64,
            max: FONT_SIZE_MAX as f64,
            step: 1.,
        },
        |cx| cx.global::<crate::config::Config>().terminal_font_size() as f64,
        |size, cx| {
            let size = size as f32;
            Theme::global_mut(cx).mono_font_size = px(size);
            update_config(move |c, _| c.terminal_font_size = Some(size), cx);
        },
    )
    .default_value(TERMINAL_FONT_SIZE_DEFAULT as f64)
}

/// Terminal scrollback cap (lines): a spinner field. Applies to newly
/// spawned sessions — an existing grid keeps the cap it was built with.
pub(super) fn terminal_scrollback_field() -> SettingField<f64> {
    SettingField::<f64>::number_input(
        NumberFieldOptions {
            min: 100.,
            max: 100_000.,
            step: 500.,
        },
        |cx| cx.global::<crate::config::Config>().terminal_scrollback() as f64,
        |lines, cx| {
            update_config(move |c, _| c.terminal_scrollback = Some(lines as usize), cx)
        },
    )
    .default_value(TERMINAL_SCROLLBACK_DEFAULT as f64)
}

pub(super) fn terminal_font_field() -> SettingField<SharedString> {
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
pub(super) fn is_mono_family(family: &str) -> bool {
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
