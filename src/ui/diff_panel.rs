//! Right pane: the selected file's hunks. Header: branch + change
//! stats. The file tree lives in the sidebar's lower layer
//! ([`super::diff_tree`]); this pane always shows the current
//! selection. Diff lines never truncate — long lines scroll
//! horizontally with a visible scrollbar.

use super::PANEL_HEADER_PX;
use super::diff_tree::plus_minus;
use crate::app::AppView;
use crate::diff::{DiffFile, DiffLine};
use gpui_kit::base::SelectableText;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub(crate) fn render(
    this: &AppView,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    // The file strip only makes sense with a selected file — without
    // one the body's empty state carries the panel on its own.
    let has_file = this
        .diff
        .as_ref()
        .is_some_and(|d| !d.is_empty() && this.diff_file.is_some_and(|ix| ix < d.files.len()));
    v_flex()
        .h_full()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .bg(cx.theme().background)
        // Clicking anywhere in the panel moves focus off the terminal,
        // so its block cursor turns hollow.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.window_focus.focus(window, cx)),
        )
        .when(has_file, |el| el.child(header(this, cx)))
        .child(body(this, window, cx))
}

fn header(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    // The pane shows exactly one file: the tree selection. Its path
    // leads the header, its +/- figures close the line. The totals
    // ("N files changed") live atop the tree layer.
    let file = this
        .diff
        .as_ref()
        .and_then(|d| this.diff_file.and_then(|ix| d.files.get(ix)));

    div()
        .h(px(PANEL_HEADER_PX))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_sm()
                .font_medium()
                .text_color(cx.theme().foreground.opacity(0.9))
                .child(file.map(|f| f.path.clone()).unwrap_or_default()),
        )
        .child(div().flex_1())
        .when_some(file, |el, f| el.child(plus_minus(f.added, f.removed, cx)))
}

fn body(this: &AppView, window: &mut Window, cx: &mut Context<AppView>) -> impl IntoElement {
    // The pane mirrors the tree: no active session → no diff content.
    if this.current_session().is_none() {
        return empty("No active session — select one in the project tree.", cx)
            .into_any_element();
    }
    let Some(diff) = &this.diff else {
        return empty(this.diff_error.as_deref().unwrap_or("Loading changes…"), cx)
            .into_any_element();
    };
    if diff.is_empty() {
        return empty("No changes — working tree clean.", cx).into_any_element();
    }
    // No selection, no content: the pane stays empty until a tree
    // click picks a file.
    let Some(file_ix) = this.diff_file.filter(|ix| *ix < diff.files.len()) else {
        return empty("Select a file in the tree.", cx).into_any_element();
    };

    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .child(
            div()
                .id("diff-hunks")
                .size_full()
                // Cross-axis alignment: default stretch would clamp the
                // content column to the viewport width, so taffy would
                // report content_size == viewport and the horizontal
                // scrollbar would never get a range.
                .items_start()
                .overflow_scroll()
                .track_scroll(&this.diff_hunks_scroll)
                .p_2()
                .child(file_diff(&diff.files[file_ix], window, cx)),
        )
        .scrollbar(&this.diff_hunks_scroll, scroll::ScrollbarAxis::Both)
        .context_menu({
            let path = diff.files[file_ix].path.clone();
            let lines = diff.files[file_ix]
                .hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| l.kind != '-')
                .map(|l| l.text.clone())
                .collect::<Vec<_>>();
            move |menu, _, _| {
                let path = path.clone();
                let contents = lines.join("\n");
                menu.item(
                    PopupMenuItem::new("Copy File Path")
                        .icon(Icon::new(IconName::Copy))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                        }),
                )
                .item(
                    PopupMenuItem::new("Copy File Contents")
                        .icon(Icon::new(IconName::Copy))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(contents.clone()));
                        }),
                )
            }
        })
        .into_any_element()
}

/// Longest lines shaped exactly per render (bound on text-system calls).
const MEASURE_CANDIDATES: usize = 16;
/// Chrome left of a diff line's text: two number gutters (36px each),
/// the sign column (14px) and the text block's `pl_2`/`pr_3` padding.
const LINE_CHROME: f32 = 36. + 36. + 14. + 8. + 12.;
/// Hunk header horizontal padding (`px_2` on both sides).
const HEADER_CHROME: f32 = 16.;

fn file_diff(file: &DiffFile, window: &mut Window, cx: &mut Context<AppView>) -> impl IntoElement {
    let content_w = measure_content_width(file, window, cx);
    // Explicit width + min_w_full: the scroll container derives its
    // content size from child layout bounds, and taffy fit-content-clamps
    // auto-width children to the viewport, so only a definite width gives
    // the horizontal scrollbar a range. Rows stretch to it (min_w_full),
    // keeping the +/- tint spanning the whole scrollable width.
    let mut hunks = v_flex().gap_2().w(content_w).min_w_full().flex_shrink_0();
    if file.hunks.is_empty() {
        hunks = hunks.child(super::meta_text(
            "No text changes to display (binary, empty file, or metadata change).",
            cx,
        ));
    }
    let mut line_no = 0usize;
    for hunk in &file.hunks {
        let mut h = v_flex()
            .items_start()
            .child(hunk_header(hunk.header.clone(), cx));
        for line in &hunk.lines {
            let id = line_no;
            line_no += 1;
            h = h.child(diff_line(id, line, cx));
        }
        hunks = hunks.child(h);
    }
    if file.truncated {
        hunks = hunks.child(super::meta_text(
            "Preview limited to 5,000 lines. Change totals include the entire file.",
            cx,
        ));
    }
    hunks
}

/// Width the content column needs so the longest line never clips.
/// Candidates are ranked by a display-cell estimate (non-ASCII ~2 cells),
fn diff_line(id: usize, line: &DiffLine, cx: &mut Context<AppView>) -> impl IntoElement {
    let (old, new) = (
        line.old_no.map(|n| n.to_string()).unwrap_or_default(),
        line.new_no.map(|n| n.to_string()).unwrap_or_default(),
    );
    let tint = match line.kind {
        '+' => Some(cx.theme().green.opacity(0.12)),
        '-' => Some(cx.theme().red.opacity(0.12)),
        _ => None,
    };
    let mono = cx.theme().mono_font_family.clone();
    let sign_color = match line.kind {
        '+' => cx.theme().green,
        '-' => cx.theme().red,
        _ => cx.theme().foreground.opacity(0.0),
    };
    let text = line.text.clone();

    div()
        .id(("diff-line", id))
        .flex()
        .items_start()
        .min_w_full()
        .font_family(mono.clone())
        .text_xs()
        .when_some(tint, |el, tint| el.bg(tint))
        .child(gutter(old, cx))
        .child(gutter(new, cx))
        .child(
            div()
                .w(px(14.))
                .flex_shrink_0()
                .text_color(sign_color)
                .child(line.kind.to_string()),
        )
        .child(
            div()
                .flex_shrink_0()
                .pl_2()
                .pr_3()
                // Each line's text is its own window-selection
                // participant, ordered by `document_order`, so drag
                // selection spans lines and ⌘C copies them joined
                // with newlines (no gutter or sign in the copy).
                .child(
                    SelectableText::new(("diff-text", id), text)
                        .document_order(id as u64)
                        .text_style(TextStyleRefinement {
                            font_family: Some(mono),
                            font_size: Some(rems(0.75).into()),
                            color: Some(cx.theme().foreground.opacity(0.85)),
                            white_space: Some(WhiteSpace::Nowrap),
                            ..Default::default()
                        }),
                ),
        )
}

/// Width the content column needs so the longest line never clips.
/// Candidates are ranked by a display-cell estimate (non-ASCII ~2 cells),
/// then the top few are shaped exactly with the mono font at `text_xs`.
fn measure_content_width(
    file: &DiffFile,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> Pixels {
    let font = Font {
        family: cx.theme().mono_font_family.clone(),
        ..Default::default()
    };
    let size = px(0.75 * f32::from(window.rem_size()));
    let estimate = |s: &str| {
        s.chars()
            .fold(0usize, |n, c| n + if c.is_ascii() { 1 } else { 2 })
    };

    let mut candidates: Vec<(usize, &str, f32)> = Vec::new();
    for hunk in &file.hunks {
        candidates.push((estimate(&hunk.header), hunk.header.as_str(), HEADER_CHROME));
        for line in &hunk.lines {
            candidates.push((estimate(&line.text), line.text.as_str(), LINE_CHROME));
        }
    }
    candidates.sort_unstable_by(|a, b| b.0.cmp(&a.0));

    let mut max_w = px(0.);
    for (_, text, chrome) in candidates.into_iter().take(MEASURE_CANDIDATES) {
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            color: cx.theme().foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let w = window
            .text_system()
            .shape_line(text.into(), size, &[run], None)
            .width()
            + px(chrome);
        if w > max_w {
            max_w = w;
        }
    }
    max_w
}

/// The `@@ …` hunk header as a full-width band: subtle background,
/// muted mono text, spanning the whole content column like the code
/// rows below it instead of hugging the header text.
fn hunk_header(header: String, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .min_w_full()
        .mb_1()
        .px_2()
        .py_1()
        .rounded(cx.theme().radius)
        .bg(cx.theme().foreground.opacity(0.05))
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(cx.theme().foreground.opacity(0.5))
        .child(header)
}

fn gutter(no: String, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .w(px(36.))
        .flex_shrink_0()
        .text_right()
        .pr_2()
        .text_xs()
        .pt(px(2.))
        .text_color(cx.theme().foreground.opacity(0.35))
        .child(no)
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

}
