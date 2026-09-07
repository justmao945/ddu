//! Settings window: the sidebar-based [`Settings`] surface in its own
//! native window, opened from the title-bar gear or ⌘,. General
//! (theme, default session) and Terminal (shell, font, scrollback)
//! live here; future config appends as new pages.
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::IconName;
use gpui_kit::component::Root;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage, Settings};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

mod shell;
mod theme;

// Re-exports for the submodules' `super::` paths (they render brand
// icons and tints from the ui root).
pub(super) use crate::ui::{agent_icon, agent_menu_row, agent_tint};

use self::shell::{default_session_field, shell_program_field};
use self::theme::{
    terminal_font_field, terminal_scrollback_field, terminal_size_field, theme_field,
};
pub(crate) use self::theme::bump_font_size;

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
            // ⌘+/⌘− zoom the terminal font from the settings window too.
            .on_action(|_: &crate::app::FontLarger, _, cx| bump_font_size(1., cx))
            .on_action(|_: &crate::app::FontSmaller, _, cx| bump_font_size(-1., cx))
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
                        SettingPage::new("General")
                            .header_style(&page_header_style())
                            .icon(IconName::Settings)
                            .group(
                                SettingGroup::new().title("Appearance").item(
                                    SettingItem::new("Theme", theme_field())
                                        .description("Color scheme for the interface."),
                                ),
                            )
                            .group(
                                SettingGroup::new().title("Sessions").item(
                                    SettingItem::new(
                                        "Default type",
                                        default_session_field(),
                                    )
                                    .description("What the sidebar + button creates."),
                                ),
                            ),
                    )
                    .page(
                        SettingPage::new("Terminal")
                            .header_style(&page_header_style())
                            .icon(IconName::SquareTerminal)
                            .group(
                                SettingGroup::new().title("Shell").item(
                                    SettingItem::new("Program", shell_program_field())
                                        .layout(Axis::Vertical)
                                        .description("Program for Terminal sessions."),
                                ),
                            )
                            .group(
                                SettingGroup::new().title("Font").items([
                                    SettingItem::new("Font", terminal_font_field())
                                        .layout(Axis::Vertical)
                                        .description(
                                            "Typeface for all sessions. System default uses the platform monospace font.",
                                        ),
                                    SettingItem::new("Size", terminal_size_field()).description(
                                        "Mono font size for all sessions, in points (8–32). ⌘+ / ⌘− zoom anywhere.",
                                    ),
                                ]),
                            )
                            .group(
                                SettingGroup::new().title("Scrollback").item(
                                    SettingItem::new("Lines", terminal_scrollback_field())
                                        .description(
                                            "Maximum history kept per session; older lines are dropped. Applies to new sessions.",
                                        ),
                                ),
                            ),
                    ),
            )
            // Overlay layers (anchored, no layout impact): dropdown
            // menus and any dialog a component raises are hosted here,
            // same as the main window root.
            .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
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
pub(super) fn update_config(f: impl FnOnce(&mut crate::config::Config, &mut App), cx: &mut App) {
    let before = cx.global::<crate::config::Config>().clone();
    cx.update_global::<crate::config::Config, _>(f);
    if *cx.global::<crate::config::Config>() == before {
        return;
    }
    let snapshot = cx.global::<crate::config::Config>().clone();
    if let Err(err) = snapshot.save() {
        crate::config::report_error(err, cx);
    }
    cx.refresh_windows();
}

/// Zed-style page header: a prominent 16px medium title above the muted
/// group titles, without the stock header's bottom hairline.
pub(super) fn page_header_style() -> StyleRefinement {
    let mut style = StyleRefinement::default();
    style.text.font_size = Some(AbsoluteLength::from(px(16.)));
    style.text.font_weight = Some(FontWeight::MEDIUM);
    style.border_widths.bottom = Some(px(0.).into());
    style
}
