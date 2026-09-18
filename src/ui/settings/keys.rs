//! The Keys page: the app's shortcut table as settings rows, with a
//! recorder that captures the next chord the user presses and stores it
//! as an override in `settings.json`'s `keys` map.
//!
//! Capture cannot go through `on_key_down`: gpui resolves the keymap
//! first, so pressing ⌘N while recording would *spawn a session* instead
//! of being recorded. The recorder is therefore an app-level keystroke
//! interceptor — [`App::intercept_keystrokes`], which runs *before*
//! action dispatch and can stop it (`App::stop_propagation`) — armed for
//! as long as a row is recording and gated on the recorder element
//! holding focus: the keys of another window, or of a settings field the
//! user clicked into, are not chords, and the recorder also stops when it
//! loses that focus (a render-time check in [`super::SettingsWindow`], so
//! clicking anywhere else is a cancel).
//!
//! The rules a capture enforces — a command modifier, no chord another
//! command holds — live in [`crate::app::keys::captured`], so the page
//! only has to print the answer.

use super::{CONTROL_W, page_header_style, update_config};
use crate::app::keys::{self, Captured, Command};
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::label::Label;
use gpui_kit::component::setting::{
    RenderOptions, SettingGroup, SettingItem, SettingPage,
};
use gpui_kit::*;

/// The row capturing a chord, and why its last keystroke was refused.
/// Lives in [`super::SettingsWindow`] because the page itself is rebuilt
/// on every frame (a `Settings` is a builder), so it cannot hold state.
pub(crate) struct Capture {
    /// The command being rebound.
    pub id: &'static str,
    /// The recorder element's focus handle: the interceptor fires only
    /// while this holds the window's focus.
    pub focus: FocusHandle,
    /// A refused keystroke's reason, shown under the row while the user
    /// tries again.
    pub error: Option<String>,
    /// Watches keystrokes until the capture ends. Never read: dropping it
    /// with the capture is what unsubscribes the interceptor.
    _sub: Subscription,
}

/// Start recording a chord for `id`: focus the recorder element and
/// intercept this window's keystrokes until one lands, the user presses
/// escape, or the recorder loses focus.
pub(super) fn record(
    view: WeakEntity<super::SettingsWindow>,
    id: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Capture {
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    let recorder = focus.clone();
    let sub = cx.intercept_keystrokes(move |event, window, cx| {
        if window.focused(cx).as_ref() != Some(&recorder) {
            return;
        }
        let Some(view) = view.upgrade() else {
            return;
        };
        let cfg = cx.global::<crate::config::Config>().clone();
        match keys::captured(&event.keystroke, id, &cfg) {
            // A modifier press: the chord is still incomplete.
            Captured::Waiting => {}
            Captured::Cancelled => {
                cx.stop_propagation();
                view.update(cx, |window, cx| window.disarm(cx));
            }
            Captured::Refused(reason) => {
                cx.stop_propagation();
                view.update(cx, |window, cx| window.refuse(reason, cx));
            }
            Captured::Chord(chord) => {
                // The keymap must not also act on the chord we just took —
                // ⌘N would spawn a session *and* be recorded.
                cx.stop_propagation();
                update_config(
                    |config, _| {
                        config.keys.insert(id.to_string(), chord.clone());
                    },
                    cx,
                );
                view.update(cx, |window, cx| window.disarm(cx));
            }
        }
    });
    Capture {
        id,
        focus,
        error: None,
        _sub: sub,
    }
}

/// What the page has to know about the recorder: which row is recording,
/// and why its last chord was refused. The [`Capture`] beside it owns the
/// interceptor and its focus handle — the page never touches either.
#[derive(Clone, Copy)]
pub(super) struct Recording<'a> {
    pub id: &'static str,
    pub focus: &'a FocusHandle,
    pub error: Option<&'a str>,
}

/// The Keys page: one row per rebindable shortcut, grouped as the table
/// orders them, plus the chords the app keeps for itself.
pub(super) fn page(
    recording: Option<Recording<'_>>,
    view: WeakEntity<super::SettingsWindow>,
    cx: &App,
) -> SettingPage {
    let mut page = SettingPage::new("Keys")
        .header_style(&page_header_style())
        .icon(super::AppIcon::Keyboard)
        .description(
            "Click a shortcut and press the keys you want; Escape cancels. A chord the app \
             claims never reaches the terminal, so pick one the shell you run does not use.",
        );
    for group in groups() {
        let rows = keys::COMMANDS
            .iter()
            .filter(|command| command.group == group)
            .map(|command| row(command, recording, &view, cx))
            .collect::<Vec<_>>();
        page = page.group(SettingGroup::new().title(group).items(rows));
    }
    page.group(
        SettingGroup::new()
            .title("Reserved")
            .description(
                "These belong to the terminal or to a bar's own field, so they are not \
                 rebindable — the shell keeps the keys the app does not claim.",
            )
            .items(keys::RESERVED.iter().map(|reserved| {
                super::item(reserved.label, reserved.description, move |options, _, _| {
                    Button::new(SharedString::from(format!(
                        "key-fixed-{}",
                        reserved.chord.chord
                    )))
                    .outline()
                    .disabled(true)
                    .with_size(options.size())
                    .w(px(CONTROL_W))
                    .child(keys::chord_label(reserved.chord.chord))
                })
            })),
    )
}

/// The groups, in the order the table lists them — the settings page
/// reads the same order the bindings are registered in.
fn groups() -> Vec<&'static str> {
    let mut groups: Vec<&'static str> = Vec::new();
    for command in keys::COMMANDS {
        if !groups.contains(&command.group) {
            groups.push(command.group);
        }
    }
    groups
}

/// One rebindable row: the command, its current chord (or the recorder
/// while it is capturing), and the description — unless the stored
/// override is unusable or a capture was refused, which is what the
/// second line has to say then.
fn row(
    command: &'static Command,
    recording: Option<Recording<'_>>,
    view: &WeakEntity<super::SettingsWindow>,
    cx: &App,
) -> SettingItem {
    let cfg = cx.global::<crate::config::Config>();
    let shortcut = keys::shortcut(command, cfg);
    let recording = recording.filter(|recording| recording.id == command.id);
    let custom = shortcut.custom;
    let error = recording.and_then(|recording| recording.error.map(str::to_string));
    let keywords = command.description.to_string();
    let chord_keywords = shortcut.text.clone();

    let second_line = match (error.clone(), recording.is_some(), shortcut.broken.clone()) {
        (Some(error), _, _) => (error, true),
        (None, true, _) => (
            "Press the keys to bind them — Escape cancels.".to_string(),
            false,
        ),
        (None, false, Some(reason)) => (format!("{reason} — reset it, or set it again."), true),
        (None, false, None) => (command.description.to_string(), false),
    };

    let control = {
        let view = view.clone();
        let id = command.id;
        let text = shortcut.text.clone();
        let focus = recording.map(|recording| recording.focus.clone());
        move |options: &RenderOptions, _: &mut Window, cx: &mut App| {
            match focus.clone() {
                Some(focus) => recorder(id, focus, error.clone(), cx).into_any_element(),
                None => h_flex()
                    .gap_1()
                    .child(
                        Button::new(SharedString::from(format!("key-chord-{id}")))
                            .outline()
                            .with_size(options.size())
                            .w(px(CONTROL_W))
                            .tooltip("Click, then press the keys you want")
                            .accessibility_label(format!(
                                "{}: {text} — click to change",
                                command.label
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(text.clone()),
                            )
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    if let Some(view) = view.upgrade() {
                                        view.update(cx, |this, cx| this.record(id, window, cx));
                                    }
                                }
                            }),
                    )
                    // The reset slot is always rendered, so the chord
                    // buttons of customized and stock rows stay in one
                    // column.
                    .child(match custom {
                        true => Button::new(SharedString::from(format!("key-reset-{id}")))
                            .icon(IconName::Undo2)
                            .ghost()
                            .xsmall()
                            .tab_stop(false)
                            .tooltip("Reset to the built-in shortcut")
                            .accessibility_label(format!("Reset {}", command.label))
                            .on_click(move |_, _, cx| {
                                update_config(|config, _| {
                                    config.keys.remove(id);
                                }, cx);
                            })
                            .into_any_element(),
                        false => div().w(px(20.)).into_any_element(),
                    })
                    .into_any_element(),
            }
        }
    };

    SettingItem::render(move |options, window, cx| {
        let (text, bad) = second_line.clone();
        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(Label::new(command.label).text_sm())
                    .child(div().flex_none().child(control(options, window, cx))),
            )
            .child(
                div()
                    .w_full()
                    .text_sm()
                    .text_color(match bad {
                        true => cx.theme().red.opacity(0.85),
                        false => cx.theme().muted_foreground,
                    })
                    .child(text),
            )
    })
    .keywords([command.label, keywords.as_str(), chord_keywords.as_str()])
    .on_reset(
        move |cx| {
            cx.global::<crate::config::Config>()
                .keys
                .contains_key(command.id)
        },
        move |_, cx| {
            update_config(
                |config, _| {
                    config.keys.remove(command.id);
                },
                cx,
            );
        },
    )
}

/// The recorder box, while a row is capturing: the element that holds
/// focus for [`record`] and says what is expected of the user.
fn recorder(id: &str, focus: FocusHandle, error: Option<String>, cx: &App) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("key-recorder-{id}")))
        .track_focus(&focus)
        .h_6()
        .px_2()
        .w(px(CONTROL_W))
        .flex()
        .items_center()
        .justify_center()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().primary)
        .bg(cx.theme().primary.opacity(0.1))
        .text_sm()
        .text_color(match error {
            Some(_) => cx.theme().red,
            None => cx.theme().foreground,
        })
        .child(match error {
            Some(_) => "Press again…".to_string(),
            None => "Press keys…".to_string(),
        })
}

#[cfg(test)]
mod tests {
    // Named, never `use super::*`: the parent globs gpui-kit, and
    // globbing *it* brings gpui's `#[test]` attribute into scope, which
    // expands into itself (recursion limit). See the note in
    // `src/app/mod.rs`'s tests.
    use crate::config::tests::ENV_LOCK;
    use crate::config::{Config, State};
    use crate::ui::settings::SettingsWindow;
    use crate::ui::settings::keys::Recording;
    use gpui::{
        Context, Entity, IntoElement, KeyContext, Keystroke, Render, TestAppContext,
        VisualTestContext, WeakEntity, Window,
    };
    use gpui_kit::component::setting::Settings;
    use gpui_kit::gpui;

    /// The settings window in a test app, built the way `open` does it
    /// (the view as the window's root — `render_dialog_layer` no-ops
    /// without a `Root` ancestor, so no dialogs are involved here). The
    /// context is cloned out, so a test can pump the app and drive the
    /// window at the same time.
    fn settings_window(
        dispatcher: gpui::TestDispatcher,
        name: &'static str,
    ) -> (TestAppContext, Entity<SettingsWindow>, VisualTestContext) {
        let mut cx0 = TestAppContext::build(dispatcher, Some(name));
        let cx = &mut cx0;
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            cx.set_global(Config::default());
            cx.set_global(State::default());
            crate::app::keys::install(cx);
        });
        let (view, vcx) = cx.add_window_view(|window, cx| SettingsWindow::new(window, cx));
        let vcx = vcx.clone();
        (cx0, view, vcx)
    }

    /// A scratch `settings.json` for the capture to write, plus its
    /// directory. The env var is process-wide, hence [`ENV_LOCK`].
    fn scratch_settings(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ddu-keys-{tag}-{}", std::process::id()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(&path);
        unsafe { std::env::set_var("DDU_SETTINGS_PATH", &path) };
        path
    }

    fn done(dir: &std::path::Path) {
        unsafe { std::env::remove_var("DDU_SETTINGS_PATH") };
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The whole round trip the page performs: record a row, press a
    /// chord, and the override lands in `settings.json` *and* in the
    /// keymap — the builtin chord retires, the new one runs the command.
    ///
    /// This is what pins the interceptor: the recorder has to see the
    /// keystroke *before* the keymap resolves it (⌘N would otherwise
    /// spawn a session instead of being recorded), and only while the
    /// recorder element holds the window's focus.
    #[test]
    fn a_recorded_chord_is_stored_and_takes_the_command_over() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let path = scratch_settings("store");
        let dir = path.parent().unwrap().to_path_buf();

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let (mut cx0, view, mut vcx) = settings_window(dispatcher, "keys_store");
                let cx = &mut cx0;
                vcx.update(|window, cx| {
                    view.update(cx, |this, cx| this.record("new_session", window, cx));
                });
                // A chord the builtin table does not use, so "the new
                // chord works" cannot pass by accident.
                vcx.simulate_keystrokes("ctrl-alt-j");

                cx.update(|cx| {
                    assert_eq!(
                        cx.global::<Config>()
                            .keys
                            .get("new_session")
                            .map(String::as_str),
                        Some("ctrl-alt-j"),
                        "the capture writes the override"
                    );
                    let keymap = cx.key_bindings();
                    let contexts = vec![KeyContext::parse("Root").unwrap()];
                    let winners = |chord: &str| {
                        let (bindings, _) = keymap.borrow().bindings_for_input(
                            &[Keystroke::parse(chord).unwrap()],
                            &contexts,
                        );
                        bindings
                            .iter()
                            .map(|binding| binding.action().name().to_string())
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(winners("ctrl-alt-j"), vec!["ddu::NewSession"]);
                    assert!(winners("secondary-n").is_empty(), "the builtin chord is retired");
                    // …and every label that prints a chord prints this
                    // one: a tooltip pointing at a chord that no longer
                    // works is the failure this feature must not have.
                    assert_eq!(
                        crate::app::keys::accel_hint(crate::app::keys::NEW_SESSION, cx),
                        crate::app::keys::chord_label("ctrl-alt-j")
                    );
                });
                vcx.update(|_, cx| {
                    view.update(cx, |this, _| assert!(this.capture.is_none(), "recording ends"));
                });
            }),
        );

        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("ctrl-alt-j"), "{saved}");
        done(&dir);
    }

    /// A bare key is refused with the reason on the row, nothing is
    /// written, and the row keeps recording so the user can just press
    /// something else. Escape is the way out.
    #[test]
    fn a_bare_key_is_refused_and_nothing_is_written() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let path = scratch_settings("bare");
        let dir = path.parent().unwrap().to_path_buf();

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let (mut cx0, view, mut vcx) = settings_window(dispatcher, "keys_bare");
                let cx = &mut cx0;
                vcx.update(|window, cx| {
                    view.update(cx, |this, cx| this.record("new_session", window, cx));
                });
                vcx.simulate_keystrokes("j");
                cx.update(|cx| assert!(cx.global::<Config>().keys.is_empty()));
                vcx.update(|_, cx| {
                    view.update(cx, |this, _| {
                        let capture = this.capture.as_ref().expect("still recording");
                        assert!(
                            capture
                                .error
                                .as_deref()
                                .is_some_and(|error| error.contains("Ctrl")),
                            "{:?}",
                            capture.error
                        );
                    });
                });
                vcx.simulate_keystrokes("escape");
                vcx.update(|_, cx| {
                    view.update(cx, |this, _| assert!(this.capture.is_none(), "escape cancels"));
                });
            }),
        );

        assert!(!path.exists(), "nothing was written");
        done(&dir);
    }

    /// The page itself renders — every row of every group, the chord
    /// button with its reset slot, and the recorder box while a row is
    /// recording. A panic in a row closure is a settings window that
    /// cannot open, and the capture tests would not catch it: they never
    /// select the Keys page (a `Settings` renders the page it has
    /// selected, and General is first).
    #[test]
    fn the_keys_page_renders_its_rows() {
        struct Page(WeakEntity<SettingsWindow>);

        impl Render for Page {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let capture = self.0.upgrade().and_then(|window| {
                    let window = window.read(cx);
                    let capture = window.capture.as_ref()?;
                    Some(Recording {
                        id: capture.id,
                        focus: &capture.focus,
                        error: capture.error.as_deref(),
                    })
                });
                Settings::new("keys-page-under-test")
                    .page(super::page(capture, self.0.clone(), cx))
            }
        }

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let (mut cx0, view, mut vcx) = settings_window(dispatcher, "keys_page");
                let cx = &mut cx0;
                let (page, page_vcx) =
                    cx.add_window_view(|_, _| Page(view.downgrade()));
                // Both control states: the stock chord buttons, then the
                // recorder (armed through the real `record`, so the focus
                // handle it renders with is the one the interceptor watches).
                page_vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                vcx.update(|window, cx| {
                    view.update(cx, |this, cx| this.record("new_session", window, cx));
                });
                page_vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                    page.update(cx, |_, cx| cx.notify());
                });
            }),
        );
    }

    /// A rebind moves a command and a reset puts it back — through the
    /// real app keymap, because the layer `apply_overrides` builds is the
    /// thing that can get this wrong. It did: an `Unbind` outlives the
    /// layer that added it (an unbind hides every *earlier* binding of
    /// that action at that chord), so the layer has to re-state the chord
    /// a command runs on now — without that, resetting left the builtin
    /// chord dead for the rest of the session.
    #[test]
    fn a_reset_puts_the_builtin_chord_back() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let path = scratch_settings("reset");
        let dir = path.parent().unwrap().to_path_buf();
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let (mut cx0, _, _) = settings_window(dispatcher, "keys_reset");
                let cx = &mut cx0;
                let winners = |cx: &mut TestAppContext, chord: &str| {
                    let contexts = vec![KeyContext::parse("Root").unwrap()];
                    let keymap = cx.update(|cx| cx.key_bindings());
                    let (bindings, _) = keymap.borrow().bindings_for_input(
                        &[Keystroke::parse(chord).unwrap()],
                        &contexts,
                    );
                    bindings
                        .iter()
                        .map(|binding| binding.action().name().to_string())
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    winners(cx, "secondary-n"),
                    vec!["ddu::NewSession"],
                    "the builtin table is what install() put there"
                );
                cx.update(|cx| {
                    super::super::update_config(
                        |config, _| {
                            config.keys.insert("new_session".into(), "ctrl-alt-j".into());
                        },
                        cx,
                    )
                });
                assert_eq!(winners(cx, "ctrl-alt-j"), vec!["ddu::NewSession"]);
                assert!(winners(cx, "secondary-n").is_empty(), "the old chord is retired");

                cx.update(|cx| {
                    super::super::update_config(|config, _| { config.keys.remove("new_session"); }, cx)
                });
                assert_eq!(
                    winners(cx, "secondary-n"),
                    vec!["ddu::NewSession"],
                    "resetting brings the builtin chord back"
                );
                assert!(
                    winners(cx, "ctrl-alt-j").is_empty(),
                    "and the chord it was moved to is retired"
                );
            }),
        );
        done(&dir);
    }
}
