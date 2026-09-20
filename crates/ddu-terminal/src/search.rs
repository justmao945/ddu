//! Find-in-grid (⌘F): the bar's own state, the scans over the grid
//! (output-driven rescans coalesce behind a one-shot timer), and the
//! scroll that reveals the current hit.

use std::rc::Rc;
use std::time::Duration;

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::grid::Scroll as GridScroll;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::*;

use super::*;

/// Hard cap on find-bar hits; past it the counter just shows the cap.
const SEARCH_MAX_MATCHES: usize = 500;
/// Output-driven match rescans ride this one-shot timer so a stream of
/// PTY chunks (one wakeup each, up to display-link rate) rescans a few
/// times per second instead of per chunk; query edits rescan
/// immediately (keystroke rate) via `refresh_search`.
const SEARCH_RESCAN: Duration = Duration::from_millis(150);
/// One find-bar hit in the grid: an absolute line (negative =
/// history, alacritty's `Line` convention) plus a half-open
/// cell-column range the painter can turn into a rect directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermMatch {
    pub line: i32,
    pub start: usize,
    pub end: usize,
}
/// Find-bar state for one session's grid (⌘F while the terminal holds
/// focus). `open` folds the whole lifecycle: closed → the bar is
/// unmounted and `matches` stays empty.
pub struct TermSearch {
    pub open: bool,
    /// Created lazily on first open — `InputState` needs a `Window`,
    /// which `spawn` doesn't have.
    pub input: Option<Entity<InputState>>,
    /// Last scanned query; output wakeups rescan only when non-empty.
    pub query: String,
    /// `Rc` so the painter can grab the whole set per frame with a
    /// refcount bump; every rescan replaces it wholesale.
    pub matches: Rc<Vec<TermMatch>>,
    /// Index into `matches` the counter shows and the viewport targets.
    pub current: usize,
}
impl TermSearch {
    pub(super) fn new() -> Self {
        Self {
            open: false,
            input: None,
            query: String::new(),
            matches: Rc::new(Vec::new()),
            current: 0,
        }
    }
}

impl TermSession {

    /// True while the find bar's input holds focus — the terminal
    /// surface's key encoder must stay silent then, or every typed
    /// character would land in the grid AND the query box.
    pub fn search_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.search
            .input
            .as_ref()
            .is_some_and(|input| input.focus_handle(cx).is_focused(window))
    }

    /// Open (or refocus) the find bar; the whole query is selected so
    /// typing replaces it. The bar's input is created here on first
    /// open — `InputState` needs a `Window`, which `spawn` lacks — and
    /// its keystrokes rescan matches live.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.input.is_none() {
            let input = cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_placeholder("Find in terminal", window, cx);
                input
            });
            cx.subscribe(
                &input,
                |this: &mut Self, _, event: &InputEvent, cx: &mut Context<Self>| {
                    if matches!(event, InputEvent::Change) {
                        this.refresh_search(cx);
                        if !this.search.matches.is_empty() {
                            this.search.current = 0;
                            this.search_reveal(cx);
                        } else {
                            this.search.current = 0;
                            cx.notify();
                        }
                    }
                },
            )
            .detach();
            self.search.input = Some(input);
        }
        self.search.open = true;
        if let Some(input) = &self.search.input {
            input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        }
        self.refresh_search(cx);
        cx.notify();
    }

    /// Close the bar and hand focus back to the terminal surface (the
    /// session's own handle, so the cursor goes solid again and PTY
    /// keystrokes flow).
    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.open = false;
        self.search.matches = Rc::new(Vec::new());
        self.search.current = 0;
        self.search_dirty = false;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Step through matches, wrapping at both ends (Enter/Shift-Enter,
    /// ⌘G/⌘⇧G, the bar's chevrons).
    pub fn search_step(&mut self, back: bool, cx: &mut Context<Self>) {
        let len = self.search.matches.len();
        if len == 0 {
            return;
        }
        self.search.current = if back {
            (self.search.current + len - 1) % len
        } else {
            (self.search.current + 1) % len
        };
        self.search_reveal(cx);
    }

    /// Recompute matches from the input's current value against this
    /// session's grid. Called on keystrokes, open and the rescan
    /// timer; `current` stays clamped when the set shrank.
    pub(crate) fn refresh_search(&mut self, cx: &mut Context<Self>) {
        let query = self
            .search
            .input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        self.search.matches = if self.search.open {
            Rc::new(grid::search_grid(
                &self.grid.term,
                &query,
                SEARCH_MAX_MATCHES,
            ))
        } else {
            Rc::new(Vec::new())
        };
        self.search.query = query;
        self.search.current = self
            .search
            .current
            .min(self.search.matches.len().saturating_sub(1));
        cx.notify();
    }

    /// Output landed while the bar is up: mark the match set stale and
    /// (re)arm the one-shot rescan timer. Cheap no-op when closed or
    /// the query is empty, so per-chunk wakeups cost an open-check.
    pub fn note_search_dirty(&mut self, cx: &mut Context<Self>) {
        if !self.search.open || self.search.query.is_empty() {
            return;
        }
        self.search_dirty = true;
        self.arm_search_timer(cx);
    }

    fn arm_search_timer(&mut self, cx: &mut Context<Self>) {
        if self.search_timer_armed {
            return;
        }
        self.search_timer_armed = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_RESCAN).await;
            let _ = this.update(cx, |term, cx| {
                term.search_timer_armed = false;
                if term.search_dirty {
                    term.search_dirty = false;
                    term.refresh_search(cx);
                }
            });
        })
        .detach();
    }

    /// Scroll the viewport to the current hit. Already-visible hits
    /// keep the viewport put; hits above park two rows below the top
    /// (context reads downward), hits below land on the last row.
    pub(crate) fn search_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(&hit) = self.search.matches.get(self.search.current) else {
            cx.notify();
            return;
        };
        let mut term = self.grid.term.lock();
        let grid = term.grid();
        let screen = grid.screen_lines() as i32;
        let history = grid.history_size() as i32;
        let offset = grid.display_offset() as i32;
        let target = if (-offset..=screen - 1 - offset).contains(&hit.line) {
            offset
        } else if hit.line < -offset {
            (2 - hit.line).min(history)
        } else {
            (screen - 1 - hit.line).max(0)
        };
        if target != offset {
            term.scroll_display(GridScroll::Delta(target - offset));
        }
        cx.notify();
    }
}
