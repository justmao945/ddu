//! Surface regions of the shell. Each renders a piece of [`AppView`]
//! from its own `impl AppView` block; shared layout constants and the
//! status→color mapping live here.

use gpui::SharedString;
use gpui_kit::component::*;
use gpui_kit::*;

pub(crate) mod diff_panel;
pub(crate) mod diff_tree;
pub(crate) mod session_panel;
pub(crate) mod settings_window;
pub(crate) mod status_bar;
pub(crate) mod terminal;
pub(crate) mod title_bar;

/// Shared height of the session/changes panel headers (Zed: ~28px).
pub(crate) const PANEL_HEADER_PX: f32 = 32.;
/// Shared height of a selectable single-line list row.
pub(crate) const ROW_PX: f32 = 26.;

/// Dim count/meta text next to a panel label.
pub(crate) fn meta_text(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().foreground.opacity(0.45))
        .child(text.into())
}

/// Custom icons shipped from `assets/icons` and registered in
/// `main.rs::AppAssets`, drop-in usable wherever an `IconName` fits
/// (via the `IconNamed` trait).
#[derive(Clone, Copy)]
pub(crate) enum AppIcon {
    FolderPlus,
    FileCode,
    FileConfig,
    FileImage,
    FileArchive,
    FileLock,
}

impl gpui_kit::component::IconNamed for AppIcon {
    fn path(self) -> SharedString {
        match self {
            AppIcon::FolderPlus => "icons/folder-plus.svg".into(),
            AppIcon::FileCode => "icons/file-code.svg".into(),
            AppIcon::FileConfig => "icons/file-config.svg".into(),
            AppIcon::FileImage => "icons/file-image.svg".into(),
            AppIcon::FileArchive => "icons/file-archive.svg".into(),
            AppIcon::FileLock => "icons/file-lock.svg".into(),
        }
    }
}

/// Pick a file-type icon from a path's extension: code / config / image
/// / archive / lockfile / plain text, falling back to the generic file.
pub(crate) fn diff_file_icon(path: &str) -> Icon {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    const CODE: &[&str] = &[
        "rs", "go", "swift", "c", "h", "cpp", "cc", "hpp", "js", "jsx", "ts", "tsx", "py", "rb",
        "java", "kt", "php", "cs", "lua", "sh", "zsh", "fish", "bash", "sql", "css", "scss",
        "html", "vue", "svelte", "m", "mm", "zig", "dart", "ex", "exs", "hs", "clj",
    ];
    const CONFIG: &[&str] = &["json", "toml", "yaml", "yml", "ini", "cfg", "conf", "plist"];
    const IMAGE: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "icns", "bmp", "tiff",
    ];
    const ARCHIVE: &[&str] = &["zip", "tar", "gz", "bz2", "xz", "zst", "7z", "rar", "jar"];
    const LOCK: &[&str] = &["lock"];
    const TEXT: &[&str] = &["md", "txt", "log", "csv", "rst"];

    let icon: Icon = if LOCK.contains(&ext.as_str()) {
        AppIcon::FileLock.into()
    } else if IMAGE.contains(&ext.as_str()) {
        AppIcon::FileImage.into()
    } else if ARCHIVE.contains(&ext.as_str()) {
        AppIcon::FileArchive.into()
    } else if CONFIG.contains(&ext.as_str()) {
        AppIcon::FileConfig.into()
    } else if CODE.contains(&ext.as_str()) {
        AppIcon::FileCode.into()
    } else if TEXT.contains(&ext.as_str()) {
        IconName::FileText.into()
    } else {
        IconName::File.into()
    };
    icon
}

/// Brand/status icon for a session kind (`terminal`, `claude`, `codex`,
/// `omp`, custom). Shared by the sidebar menus and the settings window.
pub(crate) fn agent_icon(kind: &str) -> Icon {
    match kind {
        "terminal" => Icon::new(IconName::SquareTerminal),
        "claude" => Icon::default().path("icons/claude.svg"),
        "codex" => Icon::default().path("icons/openai.svg"),
        "omp" => Icon::default().path("icons/omp.svg"),
        _ => Icon::new(IconName::Bot),
    }
}

/// The icon tint for a session kind: brand colors for agents, muted
/// neutrals for shells and unknown customs.
pub(crate) fn agent_tint(kind: &str, cx: &App) -> Hsla {
    let fg = cx.theme().foreground;
    match kind {
        "claude" => hsla(15. / 360., 0.64, 0.5, 1.),
        "omp" => hsla(258. / 360., 0.7, 0.55, 1.),
        "codex" => fg.opacity(0.85),
        "terminal" => fg.opacity(0.6),
        _ => fg.opacity(0.55),
    }
}

/// A menu-row rendition of an agent: tinted brand mark with a breathing
/// gap before the label (stock items gap only 4px, too tight for icons).
pub(crate) fn agent_menu_row(kind: &str, label: impl Into<SharedString>, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(
            agent_icon(kind)
                .xsmall()
                .text_color(agent_tint(kind, cx))
                .into_any_element(),
        )
        .child(label.into())
        .into_any_element()
}

/// App-wide dialog footer recipe: Cancel (outline) + one danger confirm.
/// Every confirm dialog in the app (main window and settings window)
/// must use this so footers never drift apart again. `confirm_label`
/// names the destructive action ("Close Session", "Remove", ...); the
/// `on_confirm` handler must close the dialog itself
/// (`window.close_dialog(cx)`) before or after its own work.
pub(crate) fn dialog_footer(
    confirm_label: impl Into<gpui::SharedString>,
    confirm_id: impl Into<gpui::ElementId>,
    on_confirm: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    use gpui_kit::component::button::{Button, ButtonVariants as _};
    use gpui_kit::component::dialog::DialogFooter;
    DialogFooter::new()
        .child(
            Button::new("dialog-cancel")
                .label("Cancel")
                .outline()
                .small()
                .on_click(|_, window, cx| {
                    use gpui_kit::component::WindowExt as _;
                    window.close_dialog(cx)
                }),
        )
        .child(
            Button::new(confirm_id)
                .label(confirm_label)
                .danger()
                .small()
                .on_click(on_confirm),
        )
        .into_any_element()
}
/// Single-button footer for informational alerts (no choice to make).
/// Outline 'OK' mirrors `dialog_footer`'s Cancel, so alert buttons read
/// as the same family as confirm footers instead of a lone primary.
pub(crate) fn alert_ok_footer() -> gpui::AnyElement {
    use gpui_kit::component::button::Button;
    use gpui_kit::component::dialog::DialogFooter;
    DialogFooter::new()
        .child(
            Button::new("alert-ok")
                .label("OK")
                .outline()
                .small()
                .on_click(|_, window, cx| {
                    use gpui_kit::component::WindowExt as _;
                    window.close_dialog(cx)
                }),
        )
        .into_any_element()
}
/// Zed reserves accent for activity; list selection is elevated neutral.
pub(crate) fn selection_bg(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.12)
}

/// Neutral hover fill for list rows.
pub(crate) fn hover_bg(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.04)
}

/// Keep the selected row visibly selected while hovering.
pub(crate) fn selection_hover_bg(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.16)
}
