//! Settings window: the sidebar-based [`Settings`] surface in its own
//! native window, opened from the title-bar gear or ⌘,. Appearance
//! (theme) lives here; future config appends as new pages.
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::IconName;
use gpui_kit::component::Root;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::setting::{
    SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

mod agents;
mod shell;
mod theme;

// Re-exports for the submodules' `super::` paths (they render brand
// icons and tints from the ui root).
pub(super) use crate::ui::{agent_icon, agent_menu_row, agent_tint, dialog_footer};

use self::agents::{builtin_agent_groups, custom_agents_groups};
use self::shell::{default_session_field, shell_args_field, shell_program_field};
use self::theme::{terminal_font_field, theme_field};

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

/// State for [`commit_text_field`].
struct CommitFieldState {
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// The composable half of [`commit_text_field`]: an [`Input`] that
/// commits on Enter or blur, keyed by `key`, resyncing from `current`
/// whenever the field isn't focused. Returns the bare component so
/// callers can embed it in custom rows (e.g. the builtin-agent line).
pub(super) fn commit_input(
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
pub(super) fn commit_text_field(
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
/// Zed-style page header: a prominent 16px medium title above the muted
/// group titles, without the stock header's bottom hairline.
pub(super) fn page_header_style() -> StyleRefinement {
    let mut style = StyleRefinement::default();
    style.text.font_size = Some(AbsoluteLength::from(px(16.)));
    style.text.font_weight = Some(FontWeight::MEDIUM);
    style.border_widths.bottom = Some(px(0.).into());
    style
}
