//! One session's own lifecycle: spawn the child and its pump threads,
//! resize the grid with the panel, take keystrokes and paste into the
//! master, capture the resume id an agent prints, and kill/reap on the
//! way out.

use std::rc::Rc;
use std::time::Instant;

use gpui_kit::*;

use super::palette;
use super::stream::stream_interval;
use super::*;

impl TermSession {
    /// Spawn `cmd` in a fresh PTY and start its pump threads. Initial
    /// grid size is 80×24; the element's prepaint resizes it to the
    /// panel on the first frame. Scrollback comes from the user config
    /// (`Settings → Terminal → Scrollback`).
    pub fn spawn(cmd: &PtySpawn, cx: &mut App) -> anyhow::Result<Entity<Self>> {
        let (cols, rows) = (80, 24);
        // A real child is non-deterministic IO: its output — and the
        // channel closing when its pumps exit — wakes this session's
        // forwarder *from the pump thread*, which gpui's test scheduler
        // reports as "activity on thread ddu-pty-read … Your test is not
        // deterministic" in whichever test happens to be running when the
        // wake lands (one executor serves the whole test process). The
        // scheduler's per-test escape hatch is upstream's answer to
        // exactly this case — "a mix of deterministic and
        // non-deterministic async behavior, such as when interacting with
        // I/O in an otherwise deterministic test" — and being per-test it
        // weakens no other test's checks. Every test that spawns a
        // session opts in here, once, instead of each call site
        // remembering to (`harness::shutdown` still ends the pumps).
        #[cfg(test)]
        cx.background_executor().allow_parking();
        let scrollback = cx.global::<crate::config::Config>().terminal_scrollback();
        let (wake_tx, wake_rx) = async_channel::bounded::<grid::PumpMsg>(1);
        let (grid, process, pumps) = grid::spawn_session(
            cmd,
            cols,
            rows,
            wake_tx,
            palette::DefaultColors::of(gpui_kit::component::theme::Theme::global(cx)),
            scrollback,
        )?;

        let entity = cx.new(|cx| {
            cx.observe_global::<gpui_kit::component::theme::Theme>(|this: &mut Self, cx| {
                this.grid.set_default_colors(palette::DefaultColors::of(
                    gpui_kit::component::theme::Theme::global(cx),
                ));
                // The palette is resolved from the live theme at paint
                // time — wake so existing sessions repaint immediately.
                cx.emit(TermEvent::Wakeup);
            })
            .detach();
            Self {
                grid,
                process: Some(process),
                pumps: Some(pumps),
                pump_task: None,
                focus: cx.focus_handle().tab_stop(false),
                exit: None,
                resume_id: None,
                pending_resize: None,
                last_resize: Instant::now(),
                selecting: false,
                grid_bounds: Cell::new(None),
                content_bounds: Cell::new(None),
                scrollbar_drag: None,
                scrollbar_hover: false,
                scrollbar_until: None,
                scrollbar_hide_armed: false,
                mouse_held: None,
                mouse_cell: (u32::MAX, u32::MAX),
                wheel_remainder: 0.,
                marked_text: None,
                ime_cursor_bounds: Cell::new(None),
                ever_resized: false,
                flush_scheduled: false,
                scroll_remainder: 0.,
                search: TermSearch::new(),
                stream_paint_ms: Cell::new(0.),
                search_dirty: false,
                search_timer_armed: false,
            }
        });

        // Foreground pump: coalesced wakeups → notify; exit → event.
        // Stream floods are repaint-throttled (see [`stream_interval`] /
        // `STREAM_FRAME_MIN`): the first wakeup of a burst paints
        // immediately, the rest are flushed once per interval by a
        // trailing timer.
        let weak = entity.downgrade();
        let pump_task = cx.spawn(async move |cx| {
            // `last_frame` is read through the (fake-clock-aware)
            // executor clock so throttle behavior is testable; the
            // cells are single-threaded foreground state.
            let last_frame = Rc::new(Cell::new(None::<Instant>));
            let flush_armed = Rc::new(Cell::new(false));
            // The one trailing flush in flight, held for the same reason
            // as the forwarder itself: a detached timer would outlive the
            // task that armed it (and, in a test, wake a dead one).
            let mut flush_task: Option<Task<()>> = None;
            while let Ok(msg) = wake_rx.recv().await {
                match msg {
                    grid::PumpMsg::Wakeup => {
                        // Pace against what a frame costs this process:
                        // an expensive terminal paint steps the interval
                        // down (see [`stream_interval`]).
                        let interval = stream_interval(
                            weak.update(cx, |s, _| s.stream_paint_ms.get())
                                .unwrap_or(0.),
                        );
                        let now = cx.background_executor().now();
                        let prev = last_frame.get();
                        if prev.map_or(true, |t| now.duration_since(t) >= interval) {
                            last_frame.set(Some(now));
                            let _ = weak.update(cx, |_, cx| {
                                cx.notify();
                                cx.emit(TermEvent::Wakeup);
                            });
                        } else if !flush_armed.replace(true) {
                            let weak = weak.clone();
                            let last_frame = last_frame.clone();
                            let flush_armed = flush_armed.clone();
                            let delay = interval - now.duration_since(prev.unwrap());
                            // Arming a new flush supersedes the old handle
                            // (`flush_armed` allows only one in flight):
                            // dropping it is what cancels the timer.
                            flush_task.take();
                            flush_task = Some(cx.spawn(async move |cx| {
                                cx.background_executor().timer(delay).await;
                                flush_armed.set(false);
                                last_frame.set(Some(cx.background_executor().now()));
                                let _ = weak.update(cx, |_, cx| {
                                    cx.notify();
                                    cx.emit(TermEvent::Wakeup);
                                });
                            }));
                        }
                    }
                    grid::PumpMsg::Exit(code) => {
                        let _ = weak.update(cx, |s, cx| {
                            s.exit = Some(code);
                            // Agents print their session id in the
                            // banner / exit footer (`session id: …`,
                            // `--resume <id>`, `Session ID: <id>`);
                            // the last scan of the output tail finds it.
                            if s.resume_id.is_none() {
                                s.resume_id = grid::extract_resume_id(&s.grid.recent.tail());
                            }
                            cx.emit(TermEvent::Exit(code));
                            cx.notify();
                        });
                        break;
                    }
                }
            }
            anyhow::Ok(())
        });
        // Held, not detached: dropping the handle is what cancels the
        // forwarder (see [`TermSession::pump_task`]).
        entity.update(cx, |session, _| session.pump_task = Some(pump_task));

        Ok(entity)
    }

    /// Exit code once the child has been reaped.
    pub fn exit(&self) -> Option<i32> {
        self.exit
    }

    /// Agent session id for `--resume` (`None` when the child never
    /// printed one in the captured tail).
    pub fn resume_id(&self) -> Option<&str> {
        self.resume_id.as_deref()
    }

    /// Scan the recent output tail for the agent's resume id if not yet
    /// captured. Agents print it early (banner / "resume this session
    /// with …"), but only the exit path used to look — closing the
    /// window mid-run lost it. Persist calls this before reading.
    pub(crate) fn capture_resume_id(&mut self) {
        if self.resume_id.is_none() {
            self.resume_id = grid::extract_resume_id(&self.grid.recent.tail());
        }
    }

    /// Terminal-set window title (OSC 0), if any.
    pub fn title(&self) -> Option<String> {
        self.grid.meta.lock().title.clone()
    }

    /// Stage a grid+PTY resize. Applies immediately when the last resize
    /// is older than the debounce window; otherwise the latest target is
    /// flushed by a short timer — a window drag then reflows the grid at
    /// most ~7×/s instead of once per pixel.
    pub(crate) fn request_resize(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        if self.grid.size() == (cols, rows) {
            self.pending_resize = None;
            return;
        }
        self.pending_resize = Some((cols, rows));
        if !self.ever_resized || self.last_resize.elapsed() >= RESIZE_DEBOUNCE {
            self.flush_resize(cx);
        } else if !self.flush_scheduled {
            self.flush_scheduled = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RESIZE_DEBOUNCE).await;
                let _ = this.update(cx, |s, cx| s.flush_resize(cx));
            })
            .detach();
        }
    }

    fn flush_resize(&mut self, cx: &mut Context<Self>) {
        self.flush_scheduled = false;
        let Some((cols, rows)) = self.pending_resize.take() else {
            return;
        };
        self.last_resize = Instant::now();
        self.ever_resized = true;
        if self.grid.size() == (cols, rows) {
            return;
        }
        self.grid.resize(cols, rows);
        if let Some(process) = &self.process {
            process.resize(cols, rows);
        }
        cx.emit(TermEvent::Wakeup);
    }

    /// Send keystrokes/paste bytes to the child.
    pub fn write(&self, bytes: &[u8]) {
        self.grid.write(bytes);
    }

    /// Paste `text` into the child, wrapped in bracketed-paste markers
    /// when the child enabled that mode — shells then treat pasted
    /// newlines as text instead of executing them.
    pub fn paste_text(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bracketed = self
            .grid
            .term
            .lock()
            .mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        let mut bytes = Vec::with_capacity(text.len() + 12);
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.grid.write(&bytes);
        self.grid.scroll_to_bottom();
    }

    /// Scroll the viewport (positive = towards history).
    pub fn scroll_by(&mut self, lines: f32, cx: &mut Context<Self>) {
        self.scroll_remainder += lines;
        let whole = self.scroll_remainder.trunc() as i32;
        self.scroll_remainder -= whole as f32;
        // Any wheel traffic counts as scrollbar activity — even a
        // whole==0 trickle must refresh the idle window.
        self.reveal_scrollbar(cx);
        if whole != 0 {
            self.grid.scroll(whole);
            cx.emit(TermEvent::Wakeup);
        }
    }

    /// Scroll back to the live bottom.
    pub fn scroll_to_bottom(&self) {
        self.grid.scroll_to_bottom();
    }

    /// Kill the child. The exit still comes through [`TermEvent`].
    pub fn kill(&mut self) {
        if let Some(mut process) = self.process.take() {
            process.kill();
        }
    }

    /// Kill the child and wait for both pump threads to exit. The
    /// production path only kills: the pumps end on their own once the
    /// child is dead (the reader's `read` fails, the waiter's `wait`
    /// returns), and the UI thread must not wait on a process.
    ///
    /// A test must join, because a pump thread that outlives its test
    /// wakes the local foreground task from its own thread — gpui's test
    /// scheduler calls that non-determinism, and one executor serves the
    /// whole process, so the report lands on whichever test happens to
    /// be running (see `harness::shutdown`).
    ///
    /// The forwarder task dies **first**: its future owns the channel's
    /// receiver, and while that is alive the pumps' own teardown (the
    /// last sender dropping closes the channel) wakes a `!Send` task
    /// from the pump's thread — the one wake the join cannot outrun.
    /// Cancelling the task drops the receiver, so the pumps' sends find
    /// nowhere to go, and the join is then a plain thread join.
    #[cfg(test)]
    pub(crate) fn kill_and_join(&mut self) {
        self.pump_task.take();
        self.kill();
        if let Some(pumps) = self.pumps.take() {
            pumps.join();
        }
    }

    /// One control byte into the PTY (Esc/^C/^D …): ^C is SIGINT to
    /// the foreground process group, ^D EOF on the input. No-op on an
    /// already-exited child.
    pub(crate) fn ctrl(&self, byte: u8) {
        if self.exit.is_some() {
            return;
        }
        if let Some(process) = &self.process {
            process.writer().write(&[byte]);
        }
    }

    /// Encode a keystroke into PTY bytes (`None` = not ours to handle).
    pub(crate) fn encode_keystroke(keystroke: &Keystroke) -> Option<Vec<u8>> {
        input::encode(keystroke)
    }

    /// Create the grid-painting element for this session.
    pub(crate) fn element(weak: WeakEntity<Self>, focus: FocusHandle) -> impl IntoElement {
        element::TerminalElement::new(weak, focus)
    }

    /// Plant content straight into the grid (see [`grid::TermGrid::
    /// inject_bytes`]) — how a test sets up output without treating the
    /// child to a prompt.
    #[cfg(test)]
    pub(crate) fn inject_bytes(&self, bytes: &[u8]) {
        self.grid.inject_bytes(bytes);
    }
}

#[cfg(test)]
mod tests {
    // Selective imports only: `use super::*` would pull gpui's `test`
    // proc-macro re-export into scope, shadowing the built-in `#[test]`
    // and recursing forever at expansion.
    use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};

    use crate::terminal::harness::{shutdown, spawn_cat};
    use gpui_kit::{TestAppContext, gpui};



    /// Render-affecting mutations must emit `Wakeup`: the app shell
    /// repaints solely off that event, so a bare `cx.notify()` here
    /// means mouse selection / wheel scroll never become visible.
    #[test]
    fn render_mutations_emit_wakeup() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("render_mutations_emit_wakeup"));
                let cx = &mut cx0;
                let session = cx.update(spawn_cat);
                let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                cx.update(|cx| {
                    let events = events.clone();
                    cx.subscribe(&session, move |_, event: &crate::terminal::TermEvent, _| {
                        events.borrow_mut().push(event.clone());
                    })
                    .detach();
                });

                cx.update(|cx| {
                    session.update(cx, |s, cx| {
                        s.begin_selection(
                            GridPoint::new(Line(0), Column(0)),
                            Side::Left,
                            1,
                            cx,
                        );
                        s.scroll_by(2., cx);
                        s.end_selection(cx);
                    });
                });

                let log = events.borrow();
                let wakeups = log
                    .iter()
                    .filter(|e| **e == crate::terminal::TermEvent::Wakeup)
                    .count();
                assert!(
                    wakeups >= 3,
                    "begin/scroll/end each emit Wakeup, got {log:?}"
                );
                drop(log);
                shutdown(&session, cx);
            }),
        );
    }
}
