//! Right pane: the selected file's hunks. Header: file path + change
//! stats. The file tree lives in the sidebar's lower layer
//! ([`super::file_tree`]), whose summary strip carries the
//! working-tree totals; this pane always shows the current selection.
//! Diff lines never truncate — long lines scroll horizontally with a
//! visible scrollbar.
//!
//! This file is the pane's frame (header, mode switch, body); the rows it
//! virtualizes are [`rows`], the scrollbar-column strip is [`overview`], and
//! [`find_bar`] is the ⌘F overlay.

mod body;
mod find_bar;
mod overview;
mod rows;

use std::rc::Rc;
use super::{panel_header_px, scaled};
use super::plus_minus;
use super::panel_view;
use crate::app::{AppView, CopyFileContents, CopyFilePath};
use crate::diff::file_view::FileView;
use crate::diff::{
    DiffLine, NoteKind, PaneRow, RowStream, EXPAND_MAX_LINES, MAX_LINES_PER_FILE,
};
use gpui_kit::base::SelectableText;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::text::TextView;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{self, Input};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
/// The pane's layout box: the pane's own root element reads it (the
/// shell mounts this pane *uncached* — see `AppView::render`).
pub(crate) fn root_style() -> StyleRefinement {
    StyleRefinement::default()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        // Anchor for the find bar's absolute overlay.
        .relative()
        .overflow_hidden()
}
panel_view!(
    /// Right pane: the selected file's changes.
    PanelView,
    render
);
pub(crate) fn render(
    this: &AppView,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> AnyElement {
    // The file strip only makes sense with a selected file — without
    // one the body's empty state carries the panel on its own.
    // The strip only makes sense with a selected file: without one the
    // body's empty state carries the panel on its own. A clean file (the
    // listing lists them all) has a strip too — the mode toggles still
    // apply to it.
    let has_file = this.current_diff_path().is_some() && this.diff().is_some();
    let mut root = div();
    *root.style() = root_style();
    root.bg(cx.theme().background)
        // Clicking anywhere in the panel moves focus off the terminal,
        // so its block cursor goes hollow. The find bar stops its own
        // mouse downs (see `find_bar`) so this can't steal focus back
        // from the input mid-keystroke.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.window_focus.focus(window, cx)),
        )
        .when(has_file, |el| el.child(header(this, cx)))
        .child(body::body(this, window, cx))
        .when(this.diff_search.open, |el| el.child(find_bar::find_bar(this, cx)))
        .into_any_element()
}
fn header(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    // The pane shows exactly one file: the tree selection. Its path
    // leads the header, its +/- figures close the line. The totals
    // ("N files changed") live atop the tree layer.
    let path = this.current_diff_path().unwrap_or_default();
    let file = this.selected_diff_file();
    // Ellipsize the *directory*, never the file name: at the pane's
    // default width the strip is at capacity, and ".." where the file
    // should be is worse than a shortened parent. The prefix shrinks
    // and ellipsizes, the name holds its width.
    let (dir, name) = match path.rfind('/') {
        Some(ix) => (&path[..=ix], &path[ix + 1..]),
        None => ("", path),
    };

    div()
        .h(px(panel_header_px()))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_sm()
                .font_medium()
                .whitespace_nowrap()
                .when(!dir.is_empty(), |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(cx.theme().foreground.opacity(0.5))
                            .child(dir.to_string()),
                    )
                })
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(cx.theme().foreground.opacity(0.9))
                        .child(name.to_string()),
                ),
        )
        // The figures, then the view switch last: the button that changes
        // what the pane shows belongs at the far right, past the file's
        // numbers.
        //
        // No line-count chip: at the pane's default width the strip is
        // already at capacity, and a header that ellipsizes the file's
        // name to print its length has its priorities backwards (File
        // mode's gutter numbers every line anyway).
        // A file the diff did not touch has no figures to show — the
        // header then carries the path alone, rather than saying
        // "unchanged" in words the pane does not need.
        .when_some(file, |el, f| el.child(plus_minus(f.added, f.removed, cx)))
        // The switch is only offered when there is something to switch
        // to: a file nobody changed has no hunks, and a button that
        // changes nothing is a dead end.
        .when(file.is_some(), |el| el.child(mode_button(this, cx)))
}
/// The view switch: one icon button, at the far right of the strip. It
/// wears the surface it switches *to* — a document for the whole file, the
/// two-versions glyph for the hunks — so the pane says what pressing it
/// gets you, and `⌘⇧M` drives the same toggle. Labeled text buttons were
/// the wrong trade at this width: "Diff File Preview" cost the file path
/// its room to print its own name.
fn mode_button(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let (icon, label) = match this.view_mode.next() {
        ViewMode::File => (
            IconName::FileText,
            "Show the whole file".to_string(),
        ),
        _ => (IconName::Replace, "Show the diff".to_string()),
    };
    let hint = format!("{label} ({})", crate::app::accel_hint("M"));
    // The Button itself takes no debug selector (its interactivity is
    // applied to an inner element), so the box carries it for tests.
    div()
        .debug_selector(|| "view-mode-toggle-box".into())
        .child(
            Button::new("view-mode-toggle")
                .tab_stop(false)
                .icon(icon)
                .ghost()
                .small()
                .tooltip(hint.clone())
                .accessibility_label(hint)
                .on_click(
                    cx.listener(|this, _: &gpui::ClickEvent, _, cx| this.toggle_view_mode(cx)),
                ),
        )
}
/// What the user picks: the file's hunks, or the file itself. The mode
/// is per session (persisted) and both read the same [`RowStream`]
/// contract, so the find bar, `scroll_to_item` and the drag selection
/// behave identically in either.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum ViewMode {
    /// The file's hunks: what changed, nothing else.
    Diff,
    /// The whole file with the diff's changes tinted in place — the
    /// default: a file is read, not skimmed, and this surface shows the
    /// code around the change.
    #[default]
    File,
}
impl ViewMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Diff => "diff",
            Self::File => "file",
        }
    }

    /// `preview` is the pre-merge spelling of `file`: a `.md` file used
    /// to need a mode of its own, and state written back then still
    /// restores — as the whole-file mode that renders it.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "diff" => Some(Self::Diff),
            "file" | "preview" => Some(Self::File),
            _ => None,
        }
    }

    /// Where the pane's view button (and `⌘⇧M`) goes from here.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Diff => Self::File,
            Self::File => Self::Diff,
        }
    }
}
/// What the pane actually renders for the current selection. Markdown is
/// not a mode the user picks: a `.md` file *is* its rendered document in
/// File mode, and its source is what the toggle would show if it could
/// get at it (an unreadable one falls back to the rows, banded).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Surface {
    Diff,
    File,
    Preview,
}
impl Surface {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Diff => "diff",
            Self::File => "file",
            Self::Preview => "preview",
        }
    }
}
/// Which of the three the pane renders: the mode, and — for File mode —
/// whether the file is a rendered document.
pub(crate) fn surface_of(mode: ViewMode, path: Option<&str>) -> Surface {
    match mode {
        ViewMode::Diff => Surface::Diff,
        ViewMode::File if path.is_some_and(is_markdown) => Surface::Preview,
        ViewMode::File => Surface::File,
    }
}
/// The rendered-document gate.
pub(crate) fn is_markdown(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".markdown", ".mdx"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}
const NOTE_READING: &str = "Reading the file…";
/// The mode's row stream for the selected file, plus the reason the
/// requested mode could not be shown (the pane renders the diff's hunks
/// and bands the reason above them). `None` rows mean the pane has
/// nothing of its own to render — the rendered document takes the body
/// instead.
pub(crate) fn pane_rows(this: &AppView) -> (Option<RowStream<'_>>, Option<&'static str>) {
    if this.current_diff_path().is_none() {
        return (None, None);
    }
    let file = this.selected_diff_file();
    match this.surface() {
        // The rendered document's body is not rows — but a source that
        // cannot be read renders rows like File mode does, banding the
        // same reason. `None` rows here mean the Markdown view takes the
        // body; the pane's `body` asks this first.
        Surface::Preview => match (this.cached_preview(), this.preview_refusal()) {
            (Some(_), _) => (None, None),
            (None, refusal) => (
                this.file_rows(file),
                Some(refusal.map_or(NOTE_READING, |r| r.note())),
            ),
        },
        // Diff mode is only reached with a diff to show (a file nobody
        // changed renders as the file itself — see `AppView::surface`),
        // so there are no "no hunks" rows to fall back to here.
        Surface::Diff => match file {
            Some(file) => (Some(RowStream::diff(file)), None),
            None => (this.file_rows(None), None),
        },
        Surface::File => match this.cached_file_view() {
            // An image is drawn by the pane's body, not streamed as
            // rows — and it has no note, because it is not standing in
            // for anything.
            Some(FileView::Image) => (None, None),
            Some(FileView::Text(_)) => (this.file_rows(file), None),
            Some(view) => (
                this.file_rows(file),
                Some(view.refusal().map_or(NOTE_READING, |r| r.note())),
            ),
            None => (this.file_rows(file), Some(NOTE_READING)),
        },
    }
}
/// The diff-line budget in force for the selected file.
fn line_budget(this: &AppView) -> usize {
    this.current_diff_path()
        .and_then(|path| this.diff_limits.get(path).copied())
        .unwrap_or(MAX_LINES_PER_FILE)
}
/// Whether the note row's slice should grow the file's budget on sight.
fn note_expandable(this: &AppView, stream: &RowStream<'_>) -> bool {
    matches!(
        stream.note(),
        Some(NoteKind::DiffCapped | NoteKind::TintsCapped)
    ) && line_budget(this) < EXPAND_MAX_LINES
}
/// The note row's text: the stream says which note it appended, the pane
/// says what it reads like (and how far the budget has grown).
fn note_text(this: &AppView, stream: &RowStream<'_>) -> String {
    match stream.note() {
        None => String::new(),
        Some(NoteKind::NoTextChanges) => {
            "No text changes to display (binary, empty file, or metadata change).".to_owned()
        }
        Some(NoteKind::EmptyFile) => "Empty file.".to_owned(),
        Some(note @ (NoteKind::DiffCapped | NoteKind::TintsCapped)) => {
            let limit = line_budget(this);
            if limit < EXPAND_MAX_LINES {
                return if note == NoteKind::TintsCapped {
                    "Loading the file's changes…".to_owned()
                } else {
                    "Loading more lines…".to_owned()
                };
            }
            if note == NoteKind::TintsCapped {
                format!(
                    "Change tints stop at {} lines. Change totals include the entire file.",
                    grouped(limit)
                )
            } else {
                format!(
                    "Preview limited to {} lines. Change totals include the entire file.",
                    grouped(limit)
                )
            }
        }
    }
}
/// `1234567` → `1,234,567` (the truncation note's line budget).
fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
fn empty(text: &str, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_start()
        .justify_center()
        .p_4()
        .text_sm()
        .text_color(cx.theme().foreground.opacity(0.4))
        // Wrap instead of clipping when the panel is narrow.
        .child(div().max_w_full().child(text.to_string()))
}
#[cfg(test)]
mod tests {
    use gpui_kit::base::{ScrollbarMode, SelectableText, TextSelection, TextSelectionLayer};
    use gpui_kit::component::theme::Theme;
    use gpui_kit::base::v_flex;
    use gpui_kit::{
        Context, IntoElement, Modifiers, MouseButton, ParentElement, Render, Styled,
        TestAppContext, TextStyleRefinement, WhiteSpace, Window, div, gpui, point, px, red,
    };

    /// The diff pane's line text: its own selection participant with
    /// `document_order` in reading order, shaped exactly like the real
    /// rows (mono, 12px, nowrap) so layout and copy mirror the surface.
    fn line_text(order: usize, text: &str) -> impl IntoElement {
        SelectableText::new(("diff-text", order), text)
            .document_order(order as u64)
            .text_style(TextStyleRefinement {
                font_family: Some("Menlo".into()),
                font_size: Some(px(12.).into()),
                color: Some(red()),
                white_space: Some(WhiteSpace::Nowrap),
                ..Default::default()
            })
    }

    struct DiffTextRoot;

    impl Render for DiffTextRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(TextSelectionLayer).child(
                v_flex()
                    .child(div().w(px(300.)).h(px(20.)).child(line_text(0, "alpha beta")))
                    .child(div().w(px(300.)).h(px(20.)).child(line_text(1, "gamma delta"))),
            )
        }
    }

    #[test]
    fn drag_selection_spans_lines_and_copy_joins_with_newline() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("diff_selection_selectable_text"));
                let cx = &mut cx0;
                let (_view, vcx) = cx.add_window_view(|_, _| DiffTextRoot);
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });

                // Rows are pinned: line 1 at y∈[0,20), line 2 at
                // y∈[20,40). Drag from line 1 into line 2: both
                // participants project the band (the middle/mid lines
                // come out whole — per-character projection is
                // gpui-base's own contract), and the window selection
                // joins the two lines with a newline in document order.
                vcx.simulate_mouse_down(
                    point(px(2.), px(3.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );
                vcx.simulate_mouse_move(
                    point(px(150.), px(25.)),
                    Some(MouseButton::Left),
                    Modifiers::none(),
                );
                vcx.simulate_mouse_up(
                    point(px(150.), px(25.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );

                let copied = vcx.update(|window, cx| TextSelection::selected_text(window, cx));
                assert_eq!(copied, "alpha beta\ngamma delta");

                // Clicking elsewhere clears the selection.
                vcx.simulate_mouse_down(
                    point(px(150.), px(200.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );
                vcx.simulate_mouse_up(
                    point(px(150.), px(200.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );
                let cleared = vcx.update(|window, cx| TextSelection::selected_text(window, cx));
                assert_eq!(cleared, "");

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    #[test]
    fn selection_tracks_cursor_across_frame_loop() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("diff_selection_frame_loop"));
                let cx = &mut cx0;
                let (_view, vcx) = cx.add_window_view(|_, _| DiffTextRoot);
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });

                // A live drag is an interleaved (event → render) stream:
                // each render re-registers the window-level selection
                // listeners, so every move event lands and the selection
                // follows the cursor step by step. (With stale listeners
                // only the first move after a render would be handled,
                // freezing the selection until an unrelated repaint.)
                vcx.simulate_mouse_down(
                    point(px(2.), px(3.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                vcx.simulate_mouse_move(
                    point(px(60.), px(3.)),
                    Some(MouseButton::Left),
                    Modifiers::none(),
                );
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                vcx.simulate_mouse_move(
                    point(px(60.), px(25.)),
                    Some(MouseButton::Left),
                    Modifiers::none(),
                );
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                vcx.simulate_mouse_up(
                    point(px(60.), px(25.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );

                let copied = vcx.update(|window, cx| TextSelection::selected_text(window, cx));
                assert_eq!(copied, "alpha beta\ngamma delta");

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    #[test]
    fn startup_forces_hover_show_mode_on_base_scrollbars() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("scrollbar_hover_show_mode"));
                let cx = &mut cx0;
                cx.update(|cx| {
                    gpui_kit::init(cx);
                    // Mirrors main.rs startup: theme change, then the
                    // app-level scrollbar-mode override.
                    Theme::change(Theme::global(cx).mode, None, cx);
                    Theme::set_scrollbar_mode(ScrollbarMode::Hover, cx);
                });

                cx.update(|cx| {
                    assert_eq!(Theme::global(cx).scrollbar_mode, ScrollbarMode::Hover);
                    let base = gpui_kit::base::Theme::global(cx);
                    assert_eq!(base.scrollbar.mode(), ScrollbarMode::Hover);
                });
            }),
        );
    }

    /// The mode is what the user picks, the surface is what it renders:
    /// Markdown is not a third mode, it is what File mode *is* for a `.md`
    /// file.
    #[test]
    fn file_mode_renders_markdown_and_preview_parses_as_file() {
        use super::{Surface, ViewMode, surface_of};
        assert_eq!(surface_of(ViewMode::Diff, Some("README.md")), Surface::Diff);
        assert_eq!(
            surface_of(ViewMode::File, Some("README.md")),
            Surface::Preview
        );
        assert_eq!(
            surface_of(ViewMode::File, Some("docs/Notes.MDX")),
            Surface::Preview
        );
        assert_eq!(surface_of(ViewMode::File, Some("src/main.rs")), Surface::File);
        assert_eq!(surface_of(ViewMode::File, None), Surface::File);
        // No selection, or a state file written before the merge: both
        // land on a surface that can render something.
        assert_eq!(surface_of(ViewMode::Diff, None), Surface::Diff);
        assert_eq!(ViewMode::parse("preview"), Some(ViewMode::File));
        assert_eq!(ViewMode::parse("file"), Some(ViewMode::File));
        assert_eq!(ViewMode::parse("nonsense"), None);
    }

    /// The mode switch is one toggle, wherever it is driven from: the
    /// header's far-right icon button and `⌘⇧M` call the same method, and
    /// a Markdown file renders as its document in File mode.
    #[test]
    fn the_view_toggle_switches_surface_on_a_markdown_file() {
        use crate::app::{AppView, ToggleViewMode};
        use crate::config::{Config, ShellConfig, State};
        use crate::diff::{DiffFile, GitDiff};
        use gpui::Entity;

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("view_toggle"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    // A silent shell: the test drives the render tree, and
                    // a prompt arriving on the PTY thread mid-frame trips
                    // the test scheduler.
                    cx.set_global(Config {
                        shell: ShellConfig {
                            program: "/bin/cat".into(),
                        },
                        ..Default::default()
                    });
                    cx.set_global(crate::config::LoadWarnings(vec![]));
                    cx.set_global(State::default());
                });
                let (view, mut vcx): (Entity<AppView>, _) =
                    cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        let files = vec![DiffFile {
                            path: "README.md".into(),
                            added: 1,
                            removed: 0,
                            ..Default::default()
                        }];
                        v.show_diff = true;
                        v.selection = Some(crate::app::Selection {
                            path: files[0].path.clone(),
                            changed: Some(0),
                        });
                        v.snapshot = Some(crate::diff::Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files,
                            },
                            
                        });
                        cx.notify();
                    });
                    let _ = window.draw(cx);
                });
                let mode =
                    |vcx: &mut gpui::VisualTestContext| vcx.update(|_, cx| view.read(cx).view_mode);
                let surface =
                    |vcx: &mut gpui::VisualTestContext| vcx.update(|_, cx| view.read(cx).surface());
                // The default is the whole file, so a Markdown file is
                // its rendered document before any toggle.
                assert_eq!(mode(&mut vcx), super::ViewMode::File);
                assert_eq!(surface(&mut vcx), super::Surface::Preview);
                vcx.dispatch_action(ToggleViewMode);
                assert_eq!(
                    mode(&mut vcx),
                    super::ViewMode::Diff,
                    "⌘⇧M switches to the hunks"
                );
                assert_eq!(surface(&mut vcx), super::Surface::Diff);
                vcx.dispatch_action(ToggleViewMode);
                assert_eq!(
                    mode(&mut vcx),
                    super::ViewMode::File,
                    "and back to the whole file"
                );
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// Preview's decision table, which is what the pane renders by: the
    /// Markdown view when the source is there, the file's own rows with
    /// the reason when it is not, and the reading note while the read is
    /// still in flight. A refused preview must not be an empty pane.
    #[test]
    fn preview_falls_back_to_rows_and_bands_the_reason() {
        use crate::app::AppView;
        use crate::config::{Config, State};
        use crate::diff::file_view::PreviewBuild;
        use crate::diff::read::Unreadable;
        use crate::diff::{DiffFile, DiffHunk, DiffLine, GitDiff};
        use gpui::Entity;

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("preview_refusal"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    cx.set_global(Config::default());
                    cx.set_global(crate::config::LoadWarnings(vec![]));
                    cx.set_global(State::default());
                });
                let (view, vcx): (Entity<AppView>, _) =
                    cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        let files = vec![DiffFile {
                                path: "notes.md".into(),
                                added: 1,
                                removed: 0,
                                hunks: vec![DiffHunk {
                                    header: "@@ -0,0 +1 @@".into(),
                                    lines: vec![DiffLine {
                                        kind: '+',
                                        old_no: None,
                                        new_no: Some(1),
                                        text: "# Title".into(),
                                    }],
                                }],
                            lines_total: 1,
                            truncated: false,
                        }];
                        v.selection = Some(crate::app::Selection {
                            path: files[0].path.clone(),
                            changed: Some(0),
                        });
                        v.snapshot = Some(crate::diff::Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files,
                            },
                            
                        });
                        // The file is Markdown: File mode renders it.
                        v.view_mode = super::ViewMode::File;
                        cx.notify();
                    });
                });
                let key = ("notes.md".to_owned(), 0);

                // Still reading: the diff's rows, and the pane says why.
                let (stream, note) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    let (rows, note) = super::pane_rows(v);
                    (rows.map(|s| s.rows()), note)
                });
                // The fallback is the diff's own rows: one hunk header
                // plus its one line.
                assert_eq!(stream, Some(2));
                assert_eq!(note, Some(super::NOTE_READING));

                // Refused: same rows, the refusal's own note.
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        v.preview = Some(PreviewBuild {
                            key: (key.0.clone(), v.diff_gen),
                            source: Err(Unreadable::TooLarge),
                        });
                        cx.notify();
                    });
                });
                let (stream, note) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    let (rows, note) = super::pane_rows(v);
                    (rows.map(|s| s.rows()), note)
                });
                assert_eq!(stream, Some(2));
                assert_eq!(note, Some(Unreadable::TooLarge.note()));

                // Read: the body hands the file to the Markdown view, so
                // the pane has no rows of its own to render.
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        v.preview = Some(PreviewBuild {
                            key: (key.0.clone(), v.diff_gen),
                            source: Ok("# Title".into()),
                        });
                        cx.notify();
                    });
                });
                let (stream, note, read) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    let (rows, note) = super::pane_rows(v);
                    (rows.map(|s| s.rows()), note, v.cached_preview().is_some())
                });
                assert_eq!((stream, note), (None, None));
                assert!(read);

                // A clean file: the tree lists it, so it is selectable —
                // and Diff mode shows the file itself, not an empty hunks
                // pane (the surface folds a file with no diff into File
                // mode; here the file's own view is still being read).
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        v.view_mode = super::ViewMode::Diff;
                        v.selection = Some(crate::app::Selection {
                            path: "clean.md".to_owned(),
                            changed: None,
                        });
                        v.snapshot = Some(crate::diff::Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files: Vec::new(),
                            },
                                                    });
                        cx.notify();
                    });
                });
                let (surface, note) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    let (_, note) = super::pane_rows(v);
                    (v.surface(), note)
                });
                assert_eq!(surface, super::Surface::Preview, "a clean .md document");
                assert_eq!(note, Some(super::NOTE_READING));

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
