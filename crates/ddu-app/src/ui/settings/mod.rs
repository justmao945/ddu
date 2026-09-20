//! Settings window: the sidebar-based [`Settings`] surface in its own
//! native window, opened from the title-bar gear or ⌘,. General
//! (theme, default session) and Terminal (shell, font, scrollback)
//! live here; future config appends as new pages.
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::IconName;
use gpui_kit::component::Root;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::label::Label;
use gpui_kit::component::setting::{RenderOptions, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_kit::*;

mod shell;
mod theme;
pub(super) mod keys;

// Re-exports for the submodules' `super::` paths (they render brand
// icons and tints from the ui root).
pub(super) use crate::ui::{agent_icon, agent_menu_row, agent_tint, AppIcon};

/// Width shared by every select-style control (theme, default session,
/// shell program) so the three read as one right-aligned column.
pub(super) const CONTROL_W: f32 = 220.;

/// One settings row with a uniform anatomy: the title and its control
/// share the first line (control right-aligned), and the description
/// spans the full width beneath. The stock `SettingItem::Item` keeps
/// the description beside the title on the left, so rows are rendered
/// custom — search/reset behavior stays intact via the item keywords.
pub(super) fn item<R, E>(title: &'static str, description: &'static str, control: R) -> SettingItem
where
    R: Fn(&RenderOptions, &mut Window, &mut App) -> E + 'static,
    E: IntoElement + 'static,
{
    SettingItem::render(move |options, window, cx| {
        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(Label::new(title).text_sm())
                    // `flex_none` wrapper: controls like the spinner's
                    // frame are `flex_1` and would otherwise grow to fill
                    // the row instead of their own width.
                    .child(div().flex_none().child(control(options, window, cx))),
            )
            .child(
                div()
                    .w_full()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(description),
            )
    })
    .keywords([title, description])
}

use self::shell::{default_session_item, shell_program_item};
use self::theme::{
    terminal_font_item, terminal_scrollback_item, terminal_size_item, theme_item,
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
    /// The shortcut recorder, while a Keys row is capturing a chord (see
    /// [`keys`]).
    capture: Option<keys::Capture>,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(false);
        focus.focus(window, cx);
        Self {
            focus,
            capture: None,
        }
    }

    /// Arm the shortcut recorder for a command: the next chord pressed in
    /// this window becomes its shortcut (see [`keys::record`]).
    fn record(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        self.capture = Some(keys::record(view, id, window, cx));
        cx.notify();
    }

    /// Stop recording — a chord landed, the user pressed escape, or the
    /// recorder lost focus.
    fn disarm(&mut self, cx: &mut Context<Self>) {
        if self.capture.take().is_some() {
            cx.notify();
        }
    }

    /// Keep recording, but say why the last keystroke was refused.
    fn refuse(&mut self, reason: String, cx: &mut Context<Self>) {
        if let Some(capture) = &mut self.capture {
            capture.error = Some(reason);
            cx.notify();
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A recorder that lost the window's focus — the user clicked into
        // another field, or another window came forward — is no longer
        // what the next keystroke is for; stop watching.
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| !capture.focus.is_focused(window))
        {
            self.capture = None;
        }
        div()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .track_focus(&self.focus)
            .key_context("SettingsWindow")
            .on_action(|_: &crate::app::CloseSettings, window, _| window.remove_window())
            // Menu dispatch follows the focus chain — while the settings
            // window itself is focused, "Settings…" lands here and just
            // brings the existing window forward.
            .on_action(|_: &crate::app::OpenSettings, _, cx| open(cx))
            // ⌘+/⌘− zoom the terminal font from the settings window too.
            .on_action(|_: &crate::app::FontLarger, _, cx| bump_font_size(1., cx))
            .on_action(|_: &crate::app::FontSmaller, _, cx| bump_font_size(-1., cx))
            .child(
                Settings::new("ddu-settings")
                    // One compact control size for every field, so the
                    // custom buttons/dropdowns below match the stock ones.
                    .with_size(gpui_kit::component::Size::Small)
                    .with_group_variant(GroupBoxVariant::Outline)
                    .page(
                        SettingPage::new("General")
                            .header_style(&page_header_style())
                            .icon(IconName::Settings)
                            .group(SettingGroup::new().title("Appearance").item(theme_item()))
                            .group(
                                SettingGroup::new()
                                    .title("Sessions")
                                    .item(default_session_item()),
                            )
                    )
                    .page({
                        let view = cx.entity().downgrade();
                        let recording = self.capture.as_ref().map(|capture| keys::Recording {
                            id: capture.id,
                            focus: &capture.focus,
                            error: capture.error.as_deref(),
                        });
                        keys::page(recording, view, cx)
                    })
                    .page(
                        SettingPage::new("Terminal")
                            .header_style(&page_header_style())
                            .icon(IconName::SquareTerminal)
                            .group(
                                SettingGroup::new()
                                    .title("Shell")
                                    .item(shell_program_item()),
                            )
                            .group(
                                SettingGroup::new().title("Font").items([
                                    terminal_font_item(),
                                    terminal_size_item(),
                                ]),
                            )
                            .group(
                                SettingGroup::new()
                                    .title("Scrollback")
                                    .item(terminal_scrollback_item()),
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
pub(crate) fn button(cx: &App) -> impl IntoElement {
    Button::new("open-settings")
        .icon(IconName::Settings)
        .ghost()
        .small()
        .tab_stop(false)
        .tooltip(format!("Settings ({})", crate::app::accel_hint(crate::app::keys::OPEN_SETTINGS, cx)))
        .accessibility_label(format!("Settings ({})", crate::app::accel_hint(crate::app::keys::OPEN_SETTINGS, cx)))
        .on_click(|_, _, cx| open(cx))
}

/// Open the settings window, or bring the existing one forward — the
/// gear, ⌘, and the app-menu Settings item never spawn a duplicate.
pub(crate) fn open(cx: &mut App) {
    let existing = cx
        .try_global::<SettingsWindowSlot>()
        .and_then(|slot| slot.0);
    if let Some(handle) = existing
        // The window may be on the update stack — a menu/⌘, dispatch
        // routed to the settings window itself arrives inside its own
        // update, where `handle.update` fails ("window not found") even
        // though the window is alive. In that case it is already being
        // activated, so treat any live window as "already open" instead
        // of falling through and spawning a duplicate.
        && cx.windows().iter().any(|h| *h == handle)
    {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
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
        app_id: ddu_core::config::window_app_id(),
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
pub(super) fn update_config(f: impl FnOnce(&mut ddu_core::config::Config, &mut App), cx: &mut App) {
    let before = cx.global::<ddu_core::config::Config>().clone();
    cx.update_global::<ddu_core::config::Config, _>(f);
    if *cx.global::<ddu_core::config::Config>() == before {
        return;
    }
    let snapshot = cx.global::<ddu_core::config::Config>().clone();
    if let Err(err) = snapshot.save() {
        ddu_core::config::report_error(err, cx);
    }
    // A keyboard override is a config edit too, and the keymap is not
    // rebuilt from the config on its own (`app::keys` appends a layer —
    // a no-op when the `keys` map did not change).
    crate::app::keys::apply_overrides(cx);
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
