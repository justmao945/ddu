//! The pane's body: what fills the space under the header — the rows,
//! the rendered document, an image, or the one-line band saying why the
//! mode's own content is not on screen.

use super::overview::scroll_overview;
use super::rows::{RowHeights, gutter_width, measure_content_width, render_rows, row_sizes};
use super::*;


pub(super) fn body(this: &AppView, window: &mut Window, cx: &mut Context<AppView>) -> impl IntoElement {
    // The pane mirrors the tree: no active session → no diff content.
    if this.current_session().is_none() {
        return empty("No active session — select one in the project tree.", cx)
            .into_any_element();
    }
    let Some(diff) = this.diff() else {
        return empty(this.diff_error.as_deref().unwrap_or("Loading files…"), cx)
            .into_any_element();
    };
    let _ = diff;
    // No selection, no content: the pane stays empty until a tree
    // click picks a file.
    if this.current_diff_path().is_none() {
        return empty("Select a file in the tree.", cx).into_any_element();
    }
    // An image file is drawn as it is: File mode (or Diff mode on a
    // file nobody changed, which the surface rule folds into it).
    if this.surface() == Surface::File
        && matches!(this.cached_file_view(), Some(FileView::Image))
    {
        return image_body(this, cx).into_any_element();
    }
    // The rendered document replaces the rows only when its source is
    // actually there: a document that could not be read falls through to
    // `pane_rows`, which shows the file's rows and bands the reason.
    if this.surface() == Surface::Preview && this.cached_preview().is_some() {
        return preview_body(this).into_any_element();
    }
    let (stream, note) = pane_rows(this);
    let Some(stream) = stream else {
        // No rows at all: the note (an unchanged file in Diff mode) or
        // the generic empty state carries the pane.
        return empty(note.unwrap_or("Select a file in the tree."), cx).into_any_element();
    };
    let gutter_w = gutter_width(&stream, window, cx);
    let content_w = measure_content_width(&stream, gutter_w, window, cx);
    let heights = RowHeights::new(window);
    let sizes = Rc::new(row_sizes(&stream, heights));
    let surface = this.surface();

    v_flex()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .when_some(note, |el, note| el.child(notice_band(note, cx)))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(
                    // v_virtual_list builds only the visible slice per
                    // frame: row heights are declared up front (see
                    // `RowHeights`), so scrolling even a 200k-line file
                    // costs one range render instead of rebuilding every
                    // row. Item indices mirror the mode's `RowStream`
                    // walk (plus the note row), which is what keeps
                    // search's `scroll_to_item` landing on the right row
                    // in every mode.
                    v_virtual_list(
                        cx.entity(),
                        SharedString::from(format!("diff-rows-{}", surface.as_str())),
                        sizes.clone(),
                        move |this, range, window, cx| {
                            render_rows(
                                this, range, content_w, gutter_w, heights, window, cx,
                            )
                        },
                    )
                    .track_scroll(&this.diff_hunks_scroll)
                    .size_full()
                    .p_2(),
                )
                // The overview is an overlay, mounted before the
                // scrollbar so the scrollbar paints over it: it takes no
                // width from the rows, and the only thing it can cover
                // is a walk of the scrollbar's own track.
                .child(scroll_overview(&stream, &sizes, cx))
                .scrollbar(&this.diff_hunks_scroll, scroll::ScrollbarAxis::Both)
                .context_menu(file_menu),
        )
        .into_any_element()
}

/// The pane's copy menu, on both text surfaces (the rows and the
/// rendered document): the selection, then the file itself. Every item
/// carries the action it runs, which is what makes the menu show the
/// chord — `PopupMenu` renders an item's key hint from the action's
/// binding (`keys.rs`), so an item without one reads as a command the
/// keyboard cannot reach.
fn file_menu(menu: PopupMenu, _window: &mut Window, _cx: &mut Context<PopupMenu>) -> PopupMenu {
    let menu = menu.item(
        PopupMenuItem::new("Copy")
            .icon(Icon::new(IconName::Copy))
            .action(Box::new(input::Copy)),
    );
    menu.item(
        PopupMenuItem::new("Copy File Path")
            .icon(Icon::new(IconName::Copy))
            .action(Box::new(CopyFilePath)),
    )
    .item(
        PopupMenuItem::new("Copy File Contents")
            .icon(Icon::new(IconName::FileText))
            .action(Box::new(CopyFileContents)),
    )
}

/// The one-line band above the rows: why the pane is showing something
/// other than what the mode asked for (binary, oversized, deleted,
/// still reading).
fn notice_band(text: &str, cx: &Context<AppView>) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .px_3()
        .py_1()
        .bg(cx.theme().foreground.opacity(0.05))
        .text_xs()
        .text_color(cx.theme().foreground.opacity(0.5))
        .child(text.to_string())
}

/// The rendered document's body: the file's Markdown source, rendered by
/// gpui-component's own text view (its parser handles the GFM set —
/// tables, task lists, strikethrough). Kept to the same size cap as
/// File mode, so a runaway document cannot stall a frame.
fn preview_body(this: &AppView) -> impl IntoElement {
    let text = this.cached_preview().map(|text| text.to_owned()).unwrap_or_default();
    // Images in the document resolve against the *document's* directory.
    let base = this.preview_base();
    // The document is drawn from the state the pane keeps for this file
    // when it has one: that state is what holds the document's scroll, so
    // a document left mid-way comes back to that passage (a fresh element
    // starts at the top — the id is the same, but gpui drops a text view's
    // state the moment it is not rendered). Before the source lands there
    // is nothing to keep, and the one-off element is the same as ever.
    let view = match this.document_state() {
        Some(state) => TextView::new(state),
        None => TextView::markdown(
            SharedString::from(format!(
                "md-preview-{}",
                this.current_diff_path().unwrap_or_default()
            )),
            text,
        ),
    };
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        // The rendered document gets the same copy menu as the rows: it
        // is the surface where copying a *passage* is the point, and the
        // one that used to have no menu at all.
        .context_menu(file_menu)
        .child(
            view.selectable(true)
            .scrollable(true)
            // Headings follow the desktop's text scale, not gpui-base's
            // stock 14px base (see [`crate::ui::document_text_style`]).
            .style(crate::ui::document_text_style())
            .plugin(crate::ui::markdown::LocalImages::new(base))
            // Prose needs margins: the rows carry their own `p_2`, and a
            // document rendered flush against the pane's edges reads as
            // clipped text rather than a page. They belong on the *text
            // view*, not on the box around it: gpui lays the view's own
            // scrollbar out against its padding box (taffy resolves an
            // absolute child against the border box, padding excluded),
            // so a padded wrapper parks the thumb a padding short of the
            // pane's right border while the rows' thumb sits on it.
            .p_3(),
        )
        .into_any_element()
}

/// An image file, fitted to the pane: `Contain`, so it is never
/// distorted, centred in whatever space is left over.
fn image_body(this: &AppView, _cx: &mut Context<AppView>) -> impl IntoElement {
    let path = this
        .current_diff_path()
        .map(|path| this.diff_root().join(path))
        .unwrap_or_default();
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .p_2()
        .child(img(path).max_w_full().max_h_full().object_fit(ObjectFit::Contain))
        .into_any_element()
}
