//! Center pane: the agent terminal. M1 renders the mock transcript with
//! terminal semantics - Zed agent-panel style: each `$` command opens a
//! bordered block containing the command and its output until the next
//! prompt; `❯` lines are user input outside blocks. M2 replaces the
//! transcript source with a real PTY.

use gpui_kit::component::*;
use gpui_kit::*;

use crate::app::AppView;

pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .bg(cx.theme().background)
        .child(match this.current_session() {
            Some(session) => scroll(session.transcript.clone(), cx).into_any_element(),
            None => empty_state(cx).into_any_element(),
        })
}

enum Group {
    /// `$ command` plus output lines captured inside one bordered block.
    Block { cmd: String, out: Vec<String> },
    /// `❯ user input` - outside blocks, like Zed user messages.
    User(String),
}

fn group_lines(lines: &[String]) -> Vec<Group> {
    let mut groups = Vec::new();
    for line in lines {
        if let Some(cmd) = line.strip_prefix("$ ") {
            groups.push(Group::Block {
                cmd: cmd.to_string(),
                out: Vec::new(),
            });
        } else if let Some(input) = line.strip_prefix("> ") {
            groups.push(Group::User(input.to_string()));
        } else if let Some(Group::Block { out, .. }) = groups.last_mut() {
            out.push(line.clone());
        }
    }
    groups
}

fn scroll(lines: Vec<String>, cx: &mut Context<AppView>) -> impl IntoElement {
    let fg = cx.theme().foreground;
    let accent = cx.theme().accent;
    let border = cx.theme().border;
    let radius = cx.theme().radius;
    let mono = cx.theme().mono_font_family.clone();

    div()
        .id("terminal-scroll")
        .size_full()
        .overflow_y_scroll()
        .child(
            v_flex()
                .px_4()
                .py_3()
                .gap_3()
                .font_family(mono)
                .text_sm()
                .children(group_lines(&lines).into_iter().map(|group| match group {
                    Group::User(input) => div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(accent.opacity(0.7))
                                .child("❯"),
                        )
                        .child(div().text_color(fg).child(input))
                        .into_any_element(),
                    Group::Block { cmd, out } => v_flex()
                        .gap_1()
                        .border_1()
                        .border_color(border)
                        .rounded(radius)
                        .px_3()
                        .py_2()
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(div().flex_shrink_0().text_color(accent).child("$"))
                                .child(div().text_color(fg).child(cmd)),
                        )
                        .children(
                            out.into_iter().map(|line| {
                                div().text_color(fg.opacity(0.65)).child(line)
                            }),
                        )
                        .into_any_element(),
                })),
        )
}

fn empty_state(cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(cx.theme().foreground.opacity(0.4))
        .child("No sessions yet — press ⌘T to create one.")
}
