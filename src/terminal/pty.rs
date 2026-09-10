//! PTY process wrapper: spawn an agent command in a pseudo-terminal
//! (`portable-pty`) and expose the three handles the rest of the app
//! needs — a thread-safe writer, a resize-capable master, and a killer.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use portable_pty::{
    Child, ChildKiller, CommandBuilder, MasterPty, PtyPair, PtySize, native_pty_system,
};

/// Immutable spawn spec for one agent process (docs/DESIGN.md §9).
#[derive(Clone, Debug)]
pub struct PtySpawn {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// Thread-safe PTY input. UI keystrokes and emulator query responses
/// (`Event::PtyWrite`) both funnel through this; writes are small and
/// rare, so a mutex is plenty.
#[derive(Clone)]
pub struct PtyWriter {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl PtyWriter {
    fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(writer)),
        }
    }

    /// Write bytes to the PTY slave. A dead child makes this fail with
    /// `EPIPE`-style errors — that is the normal exit path, so errors
    /// are swallowed here.
    pub fn write(&self, bytes: &[u8]) {
        if let Ok(mut writer) = self.inner.lock() {
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }
}

/// Owns the PTY master and the child killer. The reader and the child
/// itself are handed to the caller's pump/waiter threads (see
/// [`super::grid::spawn_pump`]); the master stays here for resizes.
pub struct PtyProcess {
    writer: PtyWriter,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

type SpawnedPty = (
    PtyProcess,
    Box<dyn Read + Send>,
    Box<dyn Child + Send + Sync>,
);

impl PtyProcess {
    /// Spawn `cmd` in a fresh PTY of `cols`×`rows` cells.
    ///
    /// Returns the master handle plus the child (for the waiter thread)
    /// and master reader (for the pump thread).
    pub fn spawn(cmd: &PtySpawn, cols: u16, rows: u16) -> anyhow::Result<SpawnedPty> {
        let pty = native_pty_system();
        let PtyPair { master, slave } = pty.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // CommandBuilder seeds itself with the parent environment
        // (`get_base_env`); only the terminal-specific vars are added.
        let mut builder = CommandBuilder::new(&cmd.program);
        builder.args(cmd.args.iter());
        builder.cwd(&cmd.cwd);
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");

        let child = slave.spawn_command(builder)?;
        let reader = master.try_clone_reader()?;
        let writer = master.take_writer()?;
        let killer = child.clone_killer();

        Ok((
            Self {
                writer: PtyWriter::new(writer),
                master,
                killer,
            },
            reader,
            child,
        ))
    }

    pub fn writer(&self) -> &PtyWriter {
        &self.writer
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}

#[cfg(test)]
impl PtyWriter {
    pub(crate) fn test_writer(writer: impl Write + Send + 'static) -> Self {
        Self::new(Box::new(writer))
    }

    pub(crate) fn for_test() -> Self {
        struct Sink;
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        Self {
            inner: Arc::new(Mutex::new(Box::new(Sink))),
        }
    }
}
