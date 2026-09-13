//! The quick open's palette (⌘P): a floating search over the working
//! tree's every file.
//!
//! It is deliberately **not** part of the sidebar. The hits are a list of
//! paths and the tree is a tree — one is an answer, the other is a
//! place — and putting the first inside the second put a sidebar-sized
//! box around an answer that wants a palette's width, hid the tree while
//! it ran, and made every search a layout change. So the palette floats
//! over the workspace instead: a scrim, a card hanging from the top, and
//! the ranked hits under the field. The panels are left exactly as the
//! user left them, and committing a hit selects the file the way a click
//! in the tree does.
//!
//! Geometry is a share of the window (`TOP_SHARE`/`WIDTH_SHARE`) or a
//! scaled px — never a raw px — so the desktop text scale takes the card
//! with it, and taffy resolves the position, not a measurement of our
//! own: the palette is rendered before layout has run.

use std::rc::Rc;

use super::diff_tree::{figures, plus_minus};
use super::{diff_file_icon, hover_bg, meta_text, row_px, scaled, selection_bg};
use gpui_kit::base::input;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::AppView;

/// How far down the window the card's top sits, as a share of its
/// height: a palette hangs from the top, it does not float in the middle
/// of the screen.
const TOP_SHARE: f32 = 0.08;
/// The card's width, as a share of the window's, clamped so a narrow
/// window still has a field worth typing in and a wide one does not
/// stretch a path across the screen.
const WIDTH_SHARE: f32 = 0.46;
const MIN_WIDTH: f32 = 420.;
const MAX_WIDTH: f32 = 720.;
/// How many rows of list the card shows before it scrolls. The list is
/// capped (`FILE_SEARCH_MAX`) and virtualized, so this is only the
/// card's height — and the height of the list is stated, not left to
/// the rows: a virtual list measures nothing, so a host that leaves the
/// height to its content lays out at zero.
const VISIBLE_ROWS: usize = 11;

/// The palette, or nothing at all when it is closed.
pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> AnyElement {
    if !this.file_search.open {
        return div().into_any_element();
    }
    // The scrim takes every click that is not on the card — which is how
    // a palette is dismissed with the mouse — and, having no scroll
    // handler of its own, keeps a wheel over it from reaching the panes
    // underneath.
    div()
        .id("file-search-scrim")
        .debug_selector(|| "file-search-scrim".into())
        .absolute()
        .inset_0()
        .bg(gpui_kit::black().opacity(0.28))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_file_search(window, cx)),
        )
        .child(
            div()
                .absolute()
                .top(relative(TOP_SHARE))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(card(this, cx)),
        )
        .into_any_element()
}

/// The card itself: field, hits, and the key hints under them.
fn card(this: &AppView, cx: &mut Context<AppView>) -> AnyElement {
    let input = this.file_search.input.clone();
    let focus = input.read(cx).focus_handle(cx).clone();
    v_flex()
        .id("file-search-card")
        .debug_selector(|| "file-search-card".into())
        .role(Role::Dialog)
        .aria_label("Find file")
        // The card is what puts "FileSearch" on the dispatch path while
        // its field holds focus, which is what makes the step chords and
        // the plain arrows resolve here rather than anywhere else.
        .key_context("FileSearch")
        .track_focus(&focus)
        // A click inside the card is not a dismissal. The scrim is the
        // card's parent, so without this every click in the field would
        // close the palette it is typing into.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
        )
        .on_action(cx.listener(|this, enter: &input::Enter, window, cx| {
            if enter.shift {
                this.file_search_step(true, cx);
            } else {
                this.commit_file_search(window, cx);
            }
        }))
        .on_action(
            cx.listener(|this, _: &input::Escape, window, cx| this.close_file_search(window, cx)),
        )
        .w(relative(WIDTH_SHARE))
        .min_w(px(scaled(MIN_WIDTH)))
        .max_w(px(scaled(MAX_WIDTH)))
        .overflow_hidden()
        .rounded(cx.theme().radius)
        .bg(cx.theme().popover)
        .border_1()
        .border_color(cx.theme().border)
        .shadow_lg()
        .child(field(this, cx))
        .child(list(this, cx))
        .child(hints(cx))
        .into_any_element()
}

/// The field: the query input, the hit counter, and the three buttons
/// the find bars also carry (previous, next, close).
fn field(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let search = &this.file_search;
    let input = search.input.clone();
    let total = search.matches.len();
    let has_matches = total > 0;
    let typed = !input.read(cx).value().trim().is_empty();
    let counter: SharedString = if !typed {
        "".into()
    } else if !has_matches {
        "No results".into()
    } else {
        format!("{}/{}", search.current.min(total - 1) + 1, total).into()
    };
    let chord = crate::app::accel_hint("G");

    h_flex()
        .id("file-search-field")
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .child(
            Input::new(&input)
                .aria_label("Find file")
                .flex_1()
                .min_w_0()
                .appearance(true)
                .focus_bordered(false)
                .cleanable(true),
        )
        .child(
            div()
                .id("file-search-counter")
                .role(Role::Label)
                .aria_label(counter.clone())
                .min_w(px(scaled(46.)))
                .text_center()
                .text_xs()
                .text_color(cx.theme().foreground.opacity(if has_matches { 0.55 } else { 0.35 }))
                .child(counter),
        )
        .child(
            Button::new("file-search-prev")
                .accessibility_label(format!("Previous file ({chord} ⇧)"))
                .xsmall()
                .ghost()
                .icon(IconName::ChevronLeft)
                .disabled(!has_matches)
                .on_click(cx.listener(|this, _, _, cx| this.file_search_step(true, cx))),
        )
        .child(
            Button::new("file-search-next")
                .accessibility_label(format!("Next file ({chord})"))
                .xsmall()
                .ghost()
                .icon(IconName::ChevronRight)
                .disabled(!has_matches)
                .on_click(cx.listener(|this, _, _, cx| this.file_search_step(false, cx))),
        )
        .child(
            Button::new("file-search-close")
                .accessibility_label("Close file search")
                .xsmall()
                .ghost()
                .icon(IconName::Close)
                .on_click(cx.listener(|this, _, window, cx| this.close_file_search(window, cx))),
        )
}

/// The hits: the ranked paths, scrolling under a fixed ceiling, with the
/// cursor's row kept in view by `file_search_step`.
fn list(this: &AppView, cx: &mut Context<AppView>) -> AnyElement {
    let hits = this.file_search.matches.len();
    if hits == 0 {
        let query = this.file_search.input.read(cx).value().trim().to_owned();
        return div()
            .id("file-search-empty")
            .flex_shrink_0()
            .px_3()
            .py_2()
            .child(meta_text(
                if query.is_empty() {
                    "Type to search the working tree."
                } else {
                    "No files match."
                },
                cx,
            ))
            .into_any_element();
    }
    // A uniform row height: the list is a virtual one, so the sizes are
    // what it lays out and scrolls against.
    let sizes = Rc::new(vec![size(px(0.), px(row_px())); hits]);
    // The card is as tall as its answer: one row per hit, and at the
    // ceiling exactly `VISIBLE_ROWS` of them, with the rest scrolling
    // under the cursor.
    let list_h = row_px() * hits.min(VISIBLE_ROWS) as f32;
    v_flex()
        .relative()
        .h(px(list_h))
        .flex_shrink_0()
        .child(
            v_virtual_list(
                cx.entity(),
                "file-search-hits",
                sizes,
                |this, range, _window, cx| {
                    // One map per frame, not one scan per row: a hit
                    // wears the `+/−` figures of the file the poll found
                    // changed, and a linear search of the diff for each
                    // of 200 rows is the slow way to say "changed".
                    let changed: std::collections::HashMap<&str, (usize, usize)> = this
                        .diff()
                        .map(|diff| {
                            diff.files
                                .iter()
                                .map(|f| (f.path.as_str(), (f.added, f.removed)))
                                .collect()
                        })
                        .unwrap_or_default();
                    range
                        .filter_map(|hit| {
                            let path = this.file_search.paths.get(*this.file_search.matches.get(hit)?)?;
                            Some(hit_row(
                                hit,
                                path,
                                changed.get(path.as_str()).copied(),
                                hit == this.file_search.current,
                                cx,
                            ))
                        })
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&this.file_search.scroll)
            .size_full(),
        )
        .scrollbar(&this.file_search.scroll, scroll::ScrollbarAxis::Vertical)
        .into_any_element()
}

/// One hit: the path with its directories muted and the file's own name
/// bright — the name is what the query was about — plus the `+/−`
/// figures when the poll found the file changed. Clicking it is Enter.
fn hit_row(
    hit: usize,
    path: &str,
    changed: Option<(usize, usize)>,
    active: bool,
    cx: &mut Context<AppView>,
) -> AnyElement {
    let (dir, name) = match path.rfind('/') {
        Some(ix) => (&path[..=ix], &path[ix + 1..]),
        None => ("", path),
    };
    let hov_bg = hover_bg(cx);
    let active_bg = selection_bg(cx);
    div()
        .id(("file-search-hit", hit))
        .role(Role::ListItem)
        .aria_label(SharedString::from(match changed {
            Some((added, removed)) => format!("{path}{}", figures(added, removed, ' ')),
            None => path.to_owned(),
        }))
        .aria_selected(active)
        .w_full()
        .h(px(row_px()))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .cursor_pointer()
        .map(|el| if active { el.bg(active_bg) } else { el })
        .hover(move |el| el.bg(if active { active_bg } else { hov_bg }))
        .on_click(cx.listener(move |this, _, window, cx| {
            // Clicking a hit is Enter on it: the cursor moves there and
            // the pick commits.
            this.file_search.current = hit;
            this.commit_file_search(window, cx);
        }))
        .child(
            diff_file_icon(path)
                .with_size(gpui_kit::component::Size::XSmall)
                .text_color(cx.theme().foreground.opacity(0.6)),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_sm()
                .whitespace_nowrap()
                .when(!dir.is_empty(), |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(cx.theme().foreground.opacity(0.45))
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
        .when_some(changed, |el, (added, removed)| {
            el.when(added > 0 || removed > 0, |el| {
                el.child(plus_minus(added, removed, cx))
            })
        })
        .into_any_element()
}

/// The line under the list: the keys that work from anywhere in the
/// palette. `↑↓` come first because they are what a palette teaches —
/// the field is single-line, so it has no use for them itself.
fn hints(cx: &Context<AppView>) -> impl IntoElement {
    h_flex()
        .flex_shrink_0()
        .gap_3()
        .px_2()
        .py_1()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(meta_text("↑↓ move", cx))
        .child(meta_text("↵ open", cx))
        .child(meta_text("esc close", cx))
}
