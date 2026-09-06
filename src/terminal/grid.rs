//! Terminal grid: an `alacritty_terminal::Term` behind a `FairMutex`,
//! plus the two pump threads that drive it — a reader that parses PTY
//! bytes into the grid and a waiter that reports the exit status.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener};

use parking_lot::Mutex;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use portable_pty::Child;

use super::pty::{PtyProcess, PtySpawn, PtyWriter};

/// Messages from the pump threads to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpMsg {
    /// Grid or meta changed; the UI should re-render. Sent through a
    /// capacity-1 channel so bursts coalesce into at most one pending
    /// wakeup.
    Wakeup,
    /// Child exited. `i32` is the raw exit code (`-1` when wait failed).
    Exit(i32),
}

/// Escape-sequence metadata extracted from the stream (window title).
#[derive(Default)]
pub struct TermMeta {
    pub title: Option<String>,
}

/// Event sink installed into the `Term`. Query responses (`PtyWrite`)
/// are routed back to the PTY; everything user-visible becomes a
/// coalesced wakeup.
#[derive(Clone)]
pub struct EventProxy {
    writer: PtyWriter,
    wake: async_channel::Sender<PumpMsg>,
    meta: Arc<Mutex<TermMeta>>,
    dark: Arc<AtomicBool>,
}

impl EventProxy {
    fn wake(&self) {
        // Dropping when the channel is full IS the coalescing.
        let _ = self.wake.try_send(PumpMsg::Wakeup);
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::ColorRequest(index, format) => {
                if let Some(color) = query_color(index, self.dark.load(Ordering::Relaxed)) {
                    self.writer.write(format(color).as_bytes());
                }
            }
            Event::PtyWrite(text) => self.writer.write(text.as_bytes()),
            Event::Title(title) => {
                self.meta.lock().title = Some(title);
                self.wake();
            }
            Event::ResetTitle => {
                self.meta.lock().title = None;
                self.wake();
            }
            Event::Wakeup
            | Event::Bell
            | Event::MouseCursorDirty
            | Event::CursorBlinkingChange => self.wake(),
            // Cell-size (CSI 14t/18t) and clipboard (OSC 52) queries are
            // not answered yet; agent CLIs do not depend on them.
            _ => {}
        }
    }
}

/// Column/row count handed to `Term::new`/`Term::resize`.
#[derive(Clone, Copy)]
struct GridDims {
    cols: u16,
    rows: u16,
}

impl Dimensions for GridDims {
    fn columns(&self) -> usize {
        self.cols as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
}

/// The whole terminal: grid data model + escape-sequence parser feed
/// point + input writer. Shared between the UI thread (render, input,
/// resize) and the pump thread (parse).
pub struct TermGrid {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    pub meta: Arc<Mutex<TermMeta>>,
    pub writer: PtyWriter,
    /// Epoch ms of the last PTY read; drives the "agent is working" spinner.
    pub activity: Arc<AtomicU64>,
    pub dark: Arc<AtomicBool>,
    cols: u16,
    rows: u16,
}

impl TermGrid {
    pub fn new(cols: u16, rows: u16, writer: PtyWriter, wake: async_channel::Sender<PumpMsg>) -> Self {
        let activity = Arc::new(AtomicU64::new(now_ms()));
        let meta = Arc::new(Mutex::new(TermMeta::default()));
        let dark = Arc::new(AtomicBool::new(true));
        let proxy = EventProxy { writer: writer.clone(), wake, meta: meta.clone(), dark: dark.clone() };
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &GridDims { cols, rows },
            proxy,
        )));
        Self { term, meta, writer, activity, dark, cols, rows }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Resize the grid model. The PTY master must be resized separately
    /// (see [`super::TermSession::resize`]) so both stay in lockstep.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.term.lock().resize(GridDims { cols, rows });
        self.cols = cols;
        self.rows = rows;
    }

    /// Send raw bytes to the child (keystrokes, paste).
    pub fn write(&self, bytes: &[u8]) {
        self.writer.write(bytes);
    }

    /// Scroll the viewport by `lines` (positive = towards history).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    /// Jump to the live bottom of the scrollback.
    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }
}
/// Spawn the two pump threads for a freshly started [`PtyProcess`].
///
/// * Reader thread: `read → parse per byte → coalesced Wakeup`, exits on
///   EOF (child closed its output).
/// * Waiter thread: blocks on `child.wait()`, then reports `Exit`.
pub fn spawn_pump(
    term: Arc<FairMutex<Term<EventProxy>>>,
    wake: async_channel::Sender<PumpMsg>,
    activity: Arc<AtomicU64>,
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
) {
    let reader_wake = wake.clone();
    std::thread::Builder::new()
        .name("ddu-pty-read".into())
        .spawn(move || {
            let mut parser: Processor<StdSyncHandler> = Processor::new();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        activity.store(now_ms(), Ordering::Relaxed);
                        {
                            let mut term = term.lock();
                            for &byte in &buf[..n] {
                                parser.advance(&mut *term, byte);
                            }
                        }
                        let _ = reader_wake.try_send(PumpMsg::Wakeup);
                    }
                }
            }
        })
        .expect("spawn pty reader thread");

    std::thread::Builder::new()
        .name("ddu-pty-wait".into())
        .spawn(move || {
            let code = child
                .wait()
                .map(|status| status.exit_code() as i32)
                .unwrap_or(-1);
            let _ = wake.send_blocking(PumpMsg::Exit(code));
        })
        .expect("spawn pty waiter thread");
}

/// Convenience: spawn a process and its pumps in one go, returning the
/// grid plus the master handle.
pub fn spawn_session(
    cmd: &PtySpawn,
    cols: u16,
    rows: u16,
    wake: async_channel::Sender<PumpMsg>,
    dark: bool,
) -> anyhow::Result<(TermGrid, PtyProcess)> {
    let (process, reader, child) = PtyProcess::spawn(cmd, cols, rows)?;
    let grid = TermGrid::new(cols, rows, process.writer().clone(), wake.clone());
    grid.dark.store(dark, Ordering::Relaxed);
    spawn_pump(grid.term.clone(), wake, grid.activity.clone(), reader, child);
    Ok((grid, process))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn visible_text(term: &FairMutex<Term<EventProxy>>) -> String {
        let term = term.lock();
        let mut text = String::new();
        let cols = term.columns();
        let mut last_line: Option<i32> = None;
        for indexed in term.renderable_content().display_iter {
            if last_line.is_some_and(|l| l != indexed.point.line.0) {
                text.push('\n');
            }
            last_line = Some(indexed.point.line.0);
            text.push(indexed.cell.c);
            let _ = cols;
        }
        text
    }

    fn wait_until(
        term: &FairMutex<Term<EventProxy>>,
        needle: &str,
        deadline: Duration,
    ) -> bool {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if visible_text(term).contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// M2 acceptance, headless: bytes → PTY → parser → grid, and the
    /// child's exit reaches the pump channel.
    #[test]
    #[cfg(unix)]
    fn pty_roundtrip_and_exit() {
        let (wake, rx) = async_channel::bounded::<PumpMsg>(1);
        let cmd = PtySpawn {
            program: "/bin/sh".into(),
            args: vec![],
            cwd: std::env::temp_dir(),
        };
        let (grid, _process) = spawn_session(&cmd, 80, 24, wake, true).expect("spawn sh");

        assert!(
            wait_until(&grid.term, "$", Duration::from_secs(5)),
            "shell prompt never appeared"
        );

        grid.write(b"echo ddu-m2-roundtrip\r");
        assert!(
            wait_until(&grid.term, "ddu-m2-roundtrip", Duration::from_secs(5)),
            "echo output never reached the grid"
        );

        grid.write(b"exit\r");
        let start = Instant::now();
        loop {
            match rx.try_recv() {
                Ok(PumpMsg::Exit(0)) => break,
                Ok(PumpMsg::Exit(code)) => panic!("unexpected exit code {code}"),
                Ok(PumpMsg::Wakeup) => {}
                Err(async_channel::TryRecvError::Empty) => {}
                Err(e) => panic!("channel closed: {e}"),
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "exit event never arrived"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
mod zsh_probe {
    use super::*;

    fn nonspace(term: &FairMutex<Term<EventProxy>>) -> String {
        let guard = term.lock();
        let content = guard.renderable_content();
        let mut t = String::new();
        for c in content.display_iter {
            t.push(c.cell.c);
        }
        drop(guard);
        t.chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn feed(bytes: &[u8]) -> String {
        let (_tx, _rx) = async_channel::bounded::<PumpMsg>(1);
        let grid = TermGrid::new(80, 24, crate::terminal::pty::PtyWriter::for_test(), _tx);
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        {
            let mut term = grid.term.lock();
            for &b in bytes {
                parser.advance(&mut *term, b);
            }
        }
        nonspace(&grid.term)
    }

    #[test]
    fn plain_text_lands() {
        let t = feed(b"hello");
        assert!(t.contains("hello"), "got {t:?}");
    }

    #[test]
    fn zsh_prompt_bytes_land() {
        let bytes: &[u8] = b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m                  \
            \r\x1b[0m\x1b[27m\x1b[24m\x1b[Jjust@Justs-MacBook-Air";
        let t = feed(bytes);
        assert!(t.contains("just@"), "got {t:?}");
    }
}
/// Current time as epoch milliseconds (PTY activity stamp).
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// OSC 10/11/12 replies use the same default colors as the painter.
fn query_color(index: usize, dark: bool) -> Option<alacritty_terminal::vte::ansi::Rgb> {
    use alacritty_terminal::vte::ansi::{NamedColor, Rgb};
    let value = if index == NamedColor::Background as usize {
        if dark { 0x282c34 } else { 0xfafafa }
    } else if index == NamedColor::Foreground as usize || index == NamedColor::Cursor as usize {
        if dark { 0xabb2bf } else { 0x2a2c33 }
    } else { return None; };
    Some(Rgb { r: (value >> 16) as u8, g: (value >> 8) as u8, b: value as u8 })
}

#[cfg(test)]
mod color_query_tests {
    use super::*;

    #[test]
    fn osc_queries_reply_and_follow_theme_changes() {
        #[derive(Clone)]
        struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let capture = Capture(Arc::new(std::sync::Mutex::new(Vec::new())));
        let (wake, _rx) = async_channel::bounded(1);
        let grid = TermGrid::new(80, 24, PtyWriter::test_writer(capture.clone()), wake);
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        for (dark, expected) in [
            (true, "\x1b]10;rgb:abab/b2b2/bfbf\x1b\\\x1b]11;rgb:2828/2c2c/3434\x1b\\"),
            (false, "\x1b]10;rgb:2a2a/2c2c/3333\x1b\\\x1b]11;rgb:fafa/fafa/fafa\x1b\\"),
        ] {
            grid.dark.store(dark, Ordering::Relaxed);
            capture.0.lock().unwrap().clear();
            for byte in b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\" {
                parser.advance(&mut *grid.term.lock(), *byte);
            }
            assert_eq!(&*capture.0.lock().unwrap(), expected.as_bytes());
        }
    }
}
