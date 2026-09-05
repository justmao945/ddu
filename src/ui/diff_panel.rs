//! Right pane: working-tree changes - file list on top, selected file's
//! diff below. Zed-style: neutral rounded row selection, dim mono line
//! numbers, tinted +/- rows, hard truncation (never wrap) on code cells.

use gpui_kit::component::*;
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;

use super::{ROW_PX, hover_bg, selection_bg};
use crate::app::AppView;
use crate::session::{DiffFile, DiffLine};

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    v_flex()
        .h_full()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .bg(cx.theme().background)
        .child(body(this, cx))
}

fn body(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let Some(session) = this.current_session() else {
        return empty("No session selected.", cx).into_any_element();
    };
    if session.diff_files.is_empty() {
        return empty("No changes — working tree clean.", cx).into_any_element();
    }

    let file_ix = this.diff_file.min(session.diff_files.len() - 1);
    let files: Vec<_> = session.diff_files.clone();

    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .flex()
        .flex_col()
        .child(file_list(&files, file_ix, cx))
        .child(
            div()
                .id("diff-hunks")
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_y_scroll()
                .p_2()
                .child(file_diff(&files[file_ix], cx)),
        )
        .into_any_element()
}

fn file_list(files: &[DiffFile], selected: usize, cx: &mut Context<AppView>) -> impl IntoElement {
    let radius = cx.theme().radius;
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);

    let mut list = v_flex().flex_shrink_0().p_2().gap_0p5();
    for (ix, f) in files.iter().enumerate() {
        let active = ix == selected;
        list = list.child(
            div()
                .id(("diff-file", ix))
                .h(px(ROW_PX))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .rounded(radius)
                .cursor_pointer()
                .map(|el| if active { el.bg(active_bg) } else { el })
                .hover(move |el| if active { el } else { el.bg(hov_bg) })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.diff_file = ix;
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(f.path.clone()),
                )
                .child(plus_minus(f.added, f.removed, cx)),
        );
    }
    list
}

fn plus_minus(added: usize, removed: usize, cx: &mut Context<AppView>) -> impl IntoElement {
    let mono = cx.theme().mono_font_family.clone();
    div()
        .flex_shrink_0()
        .flex()
        .gap_2()
        .text_xs()
        .font_family(mono)
        .child(div().text_color(cx.theme().green).child(format!("+{added}")))
        .child(div().text_color(cx.theme().red).child(format!("-{removed}")))
}

fn file_diff(file: &DiffFile, cx: &mut Context<AppView>) -> impl IntoElement {
    let mut hunks = v_flex().gap_2().min_w_0();
    for hunk in &file.hunks {
        let mut h = v_flex()
            .min_w_0()
            .child(hunk_header(hunk.header.clone(), cx));
        for line in &hunk.lines {
            h = h.child(diff_line(line, cx));
        }
        hunks = hunks.child(h);
    }
    hunks
}

fn hunk_header(header: String, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .mb_1()
        .px_2()
        .py_1()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(cx.theme().foreground.opacity(0.5))
        .child(header)
}

fn diff_line(line: &DiffLine, cx: &mut Context<AppView>) -> impl IntoElement {
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

    div()
        .flex()
        .font_family(mono)
        .text_sm()
        .when_some(tint, |el, tint| el.bg(tint))
        .child(gutter(old, cx))
        .child(gutter(new, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pl_2()
                .pr_2()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(cx.theme().foreground.opacity(0.85))
                .child(line.text.clone()),
        )
}

fn gutter(no: String, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .w(px(36.))
        .flex_shrink_0()
        .text_right()
        .text_xs()
        .pt(px(2.))
        .text_color(cx.theme().foreground.opacity(0.35))
        .child(no)
}

fn empty(text: &str, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(cx.theme().foreground.opacity(0.4))
        .child(text.to_string())
}
