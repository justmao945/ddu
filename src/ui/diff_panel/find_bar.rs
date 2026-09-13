//! The pane's ⌘F bar: a floating input over the rows' top-right corner,
//! with the match counter and the two chevrons.

use super::*;


/// The ⌘F find bar: floats over the pane's top-right so the header and
/// content never shift. Enter/Shift-Enter and Escape arrive as actions
/// dispatched by the input itself; the "DiffSearch" key context puts
/// ⌘G/⌘⇧G (bound in `app`) on the dispatch path while the input holds
/// focus, and `track_focus` keeps the context honest about when that
/// is.
pub(super) fn find_bar(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let search = &this.diff_search;
    let input = search.input.clone();
    let focus = input.read(cx).focus_handle(cx).clone();
    let total = search.matches.len();
    let has_query = !input.read(cx).value().is_empty();
    let has_matches = total > 0;
    let counter: SharedString = if !has_query {
        "".into()
    } else if !has_matches {
        "No results".into()
    } else {
        format!("{}/{}", search.current.min(total - 1) + 1, total).into()
    };

    h_flex()
        .id("diff-find-bar")
        .occlude()
        .absolute()
        .top_2()
        .right_3()
        .track_focus(&focus)
        .key_context("DiffSearch")
        // The pane root focuses the window fallback on any mouse down;
        // without stopping propagation a click into the input would
        // bounce focus right back out.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
        )
        .on_action(cx.listener(|this, enter: &input::Enter, _, cx| {
            this.diff_search_step(enter.shift, cx);
        }))
        .on_action(cx.listener(|this, _: &input::Escape, window, cx| {
            this.close_diff_search(window, cx);
        }))
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded(cx.theme().radius)
        .bg(cx.theme().popover)
        .border_1()
        .border_color(cx.theme().border)
        .shadow_lg()
        .child(
            Input::new(&input)
                .aria_label("Find in diff")
                .small()
                .w(px(180.))
                .appearance(true)
                .focus_bordered(false)
                .cleanable(true),
        )
        .child(
            div()
                .id("diff-find-counter")
                .role(Role::Label)
                .aria_label(counter.clone())
                .min_w(px(52.))
                .text_center()
                .text_xs()
                .text_color(cx.theme().foreground.opacity(if has_matches { 0.55 } else { 0.35 }))
                .child(counter),
        )
        .child(
            Button::new("diff-find-prev")
                .accessibility_label("Previous match")
                .xsmall()
                .ghost()
                .icon(IconName::ChevronLeft)
                .disabled(!has_matches)
                .on_click(cx.listener(|this, _, _, cx| this.diff_search_step(true, cx))),
        )
        .child(
            Button::new("diff-find-next")
                .accessibility_label("Next match")
                .xsmall()
                .ghost()
                .icon(IconName::ChevronRight)
                .disabled(!has_matches)
                .on_click(cx.listener(|this, _, _, cx| this.diff_search_step(false, cx))),
        )
        .child(
            Button::new("diff-find-close")
                .accessibility_label("Close find bar")
                .xsmall()
                .ghost()
                .icon(IconName::Close)
                .on_click(cx.listener(|this, _, window, cx| this.close_diff_search(window, cx))),
        )
}
