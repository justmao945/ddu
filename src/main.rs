mod app;
mod session;
mod ui;

use app::AppView;
use gpui_kit::assets::Assets;
use gpui_kit::component::Root;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::TitleBar;
use gpui_kit::*;

fn main() {
    let app = gpui_kit::application().with_assets(Assets);

    app.run(move |cx| {
        // Must be first, before using any component features.
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Light, None, cx);

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1320.), px(820.)), cx)),
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
