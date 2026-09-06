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
            "icons/wordmark-day.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/wordmark-day.svg"
            )))),
            "icons/wordmark-up.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/wordmark-up.svg"
            )))),
            "icons/folder-plus.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/folder-plus.svg"
            )))),
            "icons/omp.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/omp.svg"
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
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("[{}] {info}\n", chrono_like_timestamp());
        eprint!("{msg}");
        let _ = std::fs::write("/tmp/ddu-panic.log", &msg);
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

    app.run(move |cx| {
        // Must be first, before using any component features.
        gpui_kit::init(cx);
        let config = config::Config::load();
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
        cx.set_global(config);
        // Activate BEFORE the first window exists: the display-link start
        // guard latches on the window's occlusion state at creation, and a
        // background-launched (unactivated) process misses it — the window
        // then freezes after its first frame and no later activation
        // recovers it (see scripts/ddu-app.sh comments).
        cx.activate(true);

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(960.), px(680.)), cx)),
            window_min_size: Some(size(px(app::WINDOW_MIN_WIDTH), px(app::WINDOW_MIN_HEIGHT))),
            ..TitleBar::window_options()
        };

        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
                let view = cx.new(|cx| AppView::new(window, cx));
                // First level on the window must be a Root.
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
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
