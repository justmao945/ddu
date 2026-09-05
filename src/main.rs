mod app;
mod config;
mod diff;
mod session;
mod terminal;
mod ui;

use std::borrow::Cow;

use app::AppView;
use gpui_kit::component::Root;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::TitleBar;
use gpui_kit::*;

/// App asset source: the crate's brand icons first, then the
/// gpui-kit defaults.
struct AppAssets;

impl gpui::AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        match path {
            "icons/claude.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!("../assets/icons/claude.svg"))))
            }
            "icons/wordmark-day.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!(
                    "../assets/icons/wordmark-day.svg"
                ))))
            }
            "icons/wordmark-up.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!(
                    "../assets/icons/wordmark-up.svg"
                ))))
            }
            "icons/folder-plus.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!(
                    "../assets/icons/folder-plus.svg"
                ))))
            }
            "icons/omp.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!("../assets/icons/omp.svg"))))
            }
            _ => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

fn main() {
    let app = gpui_kit::application().with_assets(AppAssets);

    app.run(move |cx| {
        // Must be first, before using any component features.
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Light, None, cx);
        cx.set_global(config::Config::load());

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
    });
}
