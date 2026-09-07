//! Login shell and default-session configuration fields.

use super::update_config;
use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Side;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::setting::SettingField;
use gpui_kit::*;

/// Common macOS shells for the program dropdown. Only entries that
/// actually exist on this machine are listed — uninstalled shells
/// would spawn-and-die with a cryptic PTY error.
pub(super) const SHELLS: &[&str] = &[
    "/bin/zsh",
    "/bin/bash",
    "/bin/sh",
    "/opt/homebrew/bin/fish",
    "/usr/local/bin/fish",
    "/opt/homebrew/bin/nu",
    "/usr/local/bin/nu",
];

pub(super) fn installed_shells() -> Vec<&'static str> {
    SHELLS
        .iter()
        .copied()
        .filter(|s| std::path::Path::new(s).is_file())
        .collect()
}

/// Login shell picker: a fixed dropdown (there are only a handful of
/// shells), keeping the trigger and its menu one aligned control.
pub(super) fn shell_program_field() -> SettingField<SharedString> {
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


/// Default new-session type picker: Terminal / builtins.
pub(super) fn default_session_field() -> SettingField<SharedString> {
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
