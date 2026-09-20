//! What every terminal test needs and none of them owns: a minimal
//! render root so a test window has something to host, the `cat` child
//! the grid tests drive (silent — it blocks on stdin — and echoing, so
//! the rows are the test's own), and the grid's visible text.
//!
//! Imports are selective for the same reason as in the test modules:
//! `use super::*` would pull gpui's `test` proc-macro re-export into
//! scope, shadowing the built-in `#[test]` and recursing at expansion.

use gpui_kit::component::theme::Theme;
use gpui_kit::{
    App, Context, Entity, IntoElement, Render, Styled as _,
    TestAppContext, Window, div,
};

use super::{PtySpawn, TermSession};

/// Minimal root so `cx.add_window` has something to host; the calls
/// that only need a live window (no paint) render this.
pub(super) struct TestRoot;

impl Render for TestRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

/// A `cat` session in the temp dir, with the theme global a spawn needs set.
pub(super) fn spawn_cat(cx: &mut App) -> Entity<TermSession> {
    cx.set_global(Theme::default());
    TermSession::spawn(
        &PtySpawn {
            program: "cat".into(),
            args: vec![],
            cwd: std::env::temp_dir(),
        },
        TEST_SCROLLBACK,
        cx,
    )
    .expect("spawn cat")
}

/// Grid history cap for the harness sessions: the app's own default
/// (`config::TERMINAL_SCROLLBACK_DEFAULT`), which no terminal test
/// scrolls past.
const TEST_SCROLLBACK: usize = 3000;

/// End a session the way the app's close path does — kill the child —
/// and wait for its pump threads.
///
/// Every test that spawns a PTY must call this before it ends. A pump
/// thread that outlives its test wakes the *local* foreground task from
/// its own thread, and gpui's test scheduler reports that as
/// non-determinism ("Your test is not deterministic") — attributing it
/// to whichever test is running when the wake lands, because one test
/// executor serves the whole process. `cat` alone is not enough: it
/// keeps the reader blocked, so the wake comes from the thread's *exit*
/// (idle pump, teardown) or from the child's echo, and either can fall
/// inside the next test's window.
pub(super) fn shutdown(session: &Entity<TermSession>, cx: &mut TestAppContext) {
    cx.update(|cx| session.update(cx, |s, _| s.kill_and_join()));
    cx.run_until_parked();
}

/// The grid's visible text, one line per displayed row. Wide chars
/// arrive with their spacer cell, which reads as a space, so callers
/// comparing against committed text strip whitespace.
pub(super) fn grid_text(session: &Entity<TermSession>, cx: &App) -> String {
    let term = session.read(cx).grid.term.clone();
    let term = term.lock();
    let mut text = String::new();
    let mut last_line: Option<i32> = None;
    for indexed in term.renderable_content().display_iter {
        if last_line.is_some_and(|l| l != indexed.point.line.0) {
            text.push('\n');
        }
        last_line = Some(indexed.point.line.0);
        text.push(indexed.cell.c);
    }
    text
}
