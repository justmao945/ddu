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
                        ),
                )
            })
    })
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
