mod app;
mod config;
mod diff;
mod session;
mod terminal;
mod ui;

use std::borrow::Cow;

use app::AppView;
use gpui_kit::component::Root;
use gpui_kit::component::TitleBar;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

/// App asset source: the crate's brand icons first, then the
/// gpui-kit defaults.
struct AppAssets;

impl gpui::AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        match path {
            "icons/claude.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/claude.svg"
            )))),
            "icons/openai.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/openai.svg"
            )))),
            "icons/folder-plus.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/folder-plus.svg"
            )))),
            "icons/omp.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/omp.svg"
            )))),
            "icons/file-code.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/file-code.svg"
            )))),
            "icons/file-config.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/file-config.svg"
            )))),
            "icons/file-image.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/file-image.svg"
            )))),
            "icons/file-archive.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/file-archive.svg"
            )))),
            "icons/file-lock.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/file-lock.svg"
            )))),
            _ => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

/// Panics inside AppKit event callbacks cross extern "C" boundaries and
/// abort before the message is printed; tee them to a file so crashes
/// are diagnosable.
fn install_panic_logger() {
    use std::io::Write as _;
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("[{}] {info}\n", chrono_like_timestamp());
        eprint!("{msg}");
        // Append, never overwrite: a click-dispatch panic is followed
        // by a second "cannot unwind" abort across the AppKit boundary,
        // and overwriting would destroy the original message.
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/ddu-panic.log")
        {
            let _ = f.write_all(msg.as_bytes());
        }
    }));
}

/// Wall-clock seconds since startup is enough to order panics.
fn chrono_like_timestamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u128)
        .unwrap_or(0)
}

struct ExitHook(Option<(gpui::WeakEntity<AppView>, gpui::AnyWindowHandle)>);

impl gpui::Global for ExitHook {}

fn main() {
    install_panic_logger();
    let app = gpui_kit::application().with_assets(AppAssets);

    // Reopen from the dock after the window was closed: macOS calls
    // `applicationShouldHandleReopen`; gpui surfaces it as `on_reopen`.
    // Register before `run` — the method lives on `Application`.
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_main_window(cx);
        } else {
            cx.activate(true);
        }
    });

    app.run(move |cx| {
        // Must be first, before using any component features.
        gpui_kit::init(cx);
        let (config, state, warnings) = config::load_all();
        // Load-time problems (corrupt files, failed backups) go to
        // stderr: no GUI toast, so this is the durable channel.
        for w in &warnings {
            eprintln!("[ddu] {w}");
        }
        // Bundled launches lose stderr — hand the warnings to the
        // first window, which shows them in an alert dialog once.
        cx.set_global(config::LoadWarnings(warnings));
        // Keep globals loaded even if no window opens (dock reopen
        // path), so a re-created AppView reads the same settings.
        cx.set_global(config.clone());
        cx.set_global(state);
        Theme::change(
            if config.dark_theme {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            None,
            cx,
        );
        Theme::global_mut(cx).font_size = px(14.);
        // Terminal cells use the theme's mono size; `Theme::change`
        // just reset it from the registry defaults, so re-apply the
        // configured value (falls back to the stock 13px).
        Theme::global_mut(cx).mono_font_size = px(config.terminal_font_size());
        // Overlay scrollbars reveal on hover, macOS-style. The terminal's
        // custom scrollbar already always hover-reveals; the Base scrollbars
        // (diff tree, diff pane) default to `ScrollbarMode::Scrolling` (the
        // gpui-component default / OS "when scrolling" setting), which shows
        // only after a scroll — the pane's horizontal scrollbar then feels
        // undiscoverable. `Theme::change` preserves this field, so the
        // setting survives theme switches.
        Theme::set_scrollbar_mode(gpui_kit::base::ScrollbarMode::Hover, cx);
        // Where the main window lives, so the global ⌘Q fallback can
        // route through the window's confirm dialog + graceful shutdown.
        cx.set_global(ExitHook(None));

        // Application menu. The keymap binding for `Quit` (cmd-q) must
        // exist before the menu is built — the menu item resolves its
        // shortcut from the keymap — so it is registered at app level
        // here. `OpenSettings` is too: `set_menus` runs before any window
        // exists, so the per-window bindings in `AppView::new` are not yet
        // in the keymap and the Settings item would lose its ⌘, hint.
        cx.bind_keys([
            KeyBinding::new("cmd-q", app::Quit, None),
            KeyBinding::new("cmd-,", app::OpenSettings, None),
        ]);
        cx.set_menus([Menu::new("ddu").items(vec![
            MenuItem::action("Settings…", app::OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit", app::Quit),
        ])]);
        // Global quit fallback: menu dispatch may not reach the window
        // view's action handlers (focus chain), so catch `Quit` here and
        // drive the window's view directly; a plain save+exit only when
        // no window is alive.
        cx.on_action(|_: &app::Quit, cx| {
            let hook = cx.global::<ExitHook>().0.clone();
            match hook {
                Some((view, window)) => {
                    let _ = window.update(cx, |_, window, cx| {
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |v, cx| v.request_quit(window, cx));
                        }
                    });
                }
                None => {
                    if let Some(state) = cx.try_global::<crate::config::State>() {
                        if let Err(err) = state.save() {
                            eprintln!("[ddu] Failed to save state on quit: {err}");
                        }
                    }
                    std::process::exit(0);
                }
            }
        });
        // Activate BEFORE the first window exists: the display-link start
        // guard latches on the window's occlusion state at creation, and a
        // background-launched (unactivated) process misses it — the window
        // then freezes after its first frame and no later activation
        // recovers it (see scripts/make-bundle.sh comments).
        cx.activate(true);

        open_main_window(cx);

    });
}

/// Open the main window (fresh AppView) if none is alive. Window options
/// are built here — `WindowBounds::centered` needs `&mut App`, and the
/// struct itself is not `Clone` (so it can't be stashed for callbacks).
fn open_main_window(cx: &mut gpui_kit::App) {
    if !cx.windows().is_empty() {
        return;
    }
    // Restore the last window placement when its frame still lands on
    // a connected display; otherwise fall back to centered (a monitor
    // may have been unplugged since the save).
    let saved = cx
        .try_global::<config::State>()
        .and_then(|s| s.window)
        .filter(|p| placement_visible(p, cx));
    let window_bounds = match saved {
        Some(p) => {
            let frame = Bounds {
                origin: point(px(p.x), px(p.y)),
                size: size(px(p.w), px(p.h)),
            };
            match p.mode {
                config::WindowMode::Windowed => WindowBounds::Windowed(frame),
                config::WindowMode::Maximized => WindowBounds::Maximized(frame),
                config::WindowMode::Fullscreen => WindowBounds::Fullscreen(frame),
            }
        }
        None => WindowBounds::centered(size(px(960.), px(680.)), cx),
    };
    let options = WindowOptions {
        window_bounds: Some(window_bounds),
        window_min_size: Some(size(px(app::WINDOW_MIN_WIDTH), px(app::WINDOW_MIN_HEIGHT))),
        ..TitleBar::window_options()
    };
    cx.spawn(async move |cx| {
        match cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| AppView::new(window, cx));
            // The global ⌘Q fallback drives the view through this hook
            // (the window's root view is Root, so the handle alone
            // can't reach it).
            cx.global_mut::<ExitHook>().0 = Some((view.downgrade(), window.window_handle()));
            // First level on the window must be a Root.
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            Ok(_) => {}
            Err(err) => {
                eprintln!("[ddu] open_window failed: {err}");
            }
        }
    })
    .detach();
}

/// A saved frame counts as restorable only when a connected display
/// shows enough of it to grab the window (≥100×40 logical px of
/// overlap) — otherwise the window would reopen off-screen.
fn placement_visible(p: &config::WindowPlacement, cx: &gpui_kit::App) -> bool {
    let (l, t) = (p.x, p.y);
    let (r, b) = (p.x + p.w, p.y + p.h);
    cx.displays().iter().any(|d| {
        let db = d.bounds();
        let (dl, dt) = (db.origin.x.as_f32(), db.origin.y.as_f32());
        let (dr, dbt) = (dl + db.size.width.as_f32(), dt + db.size.height.as_f32());
        let overlap_w = (r.min(dr) - l.max(dl)).max(0.);
        let overlap_h = (b.min(dbt) - t.max(dt)).max(0.);
        overlap_w >= 100. && overlap_h >= 40.
    })
}
