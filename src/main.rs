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
        // Application menu. The keymap binding for `Quit` (cmd-q) must
        // exist before the menu is built — the menu item resolves its
        // shortcut from the keymap — so it is registered at app level
        // here. ⌘Q then routes through the same action → persist → exit
        // path instead of AppKit's terminate, which this app's platform
        // plumbing never completes.
        cx.bind_keys([KeyBinding::new("cmd-q", app::Quit, None)]);
        cx.set_menus([Menu::new("ddu").items(vec![MenuItem::action("Quit", app::Quit)])]);
        // Global quit fallback: menu dispatch may not reach the window
        // view's action handlers (focus chain), so catch `Quit` here.
        // The global State is kept current by AppView::persist on every
        // mutation; flushing it to disk is the last step before exit.
        cx.on_action(|_: &app::Quit, cx| {
            if let Some(state) = cx.try_global::<crate::config::State>() {
                if let Err(err) = state.save() {
                    eprintln!("[ddu] Failed to save state on quit: {err}");
                }
            }
            std::process::exit(0);
        });
        // Activate BEFORE the first window exists: the display-link start
        // guard latches on the window's occlusion state at creation, and a
        // background-launched (unactivated) process misses it — the window
        // then freezes after its first frame and no later activation
        // recovers it (see scripts/ddu-app.sh comments).
        cx.activate(true);

        open_main_window(cx);

        // Empty string (clean launcher exports it unset-as-empty) must not
        // count as "set": require a valid page index.
        if std::env::var("DDU_VERIFY_SETTINGS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .is_some()
        {
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(600))
                    .await;
                let _ = cx.update(|cx| ui::settings_window::open(cx));
            })
            .detach();
        }
    });
}

/// Open the main window (fresh AppView) if none is alive. Window options
/// are built here — `WindowBounds::centered` needs `&mut App`, and the
/// struct itself is not `Clone` (so it can't be stashed for callbacks).
fn open_main_window(cx: &mut gpui_kit::App) {
    if !cx.windows().is_empty() {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(960.), px(680.)), cx)),
        window_min_size: Some(size(px(app::WINDOW_MIN_WIDTH), px(app::WINDOW_MIN_HEIGHT))),
        ..TitleBar::window_options()
    };
    cx.spawn(async move |cx| {
        let _ = cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| AppView::new(window, cx));
            // First level on the window must be a Root.
            cx.new(|cx| Root::new(view, window, cx))
        });
    })
    .detach();
}
