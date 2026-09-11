//! Attention signals in the PTY byte stream: the "the program wants
//! you" markers terminal agents emit when they hand control back to
//! the user (docs/DESIGN.md §10).
//!
//! Three conventions are decoded:
//! * `BEL` (`\x07`) — the universal fallback. `omp` reports every
//!   finished turn, every question and every error through it when it
//!   cannot negotiate a richer channel with the terminal.
//! * `OSC 9 ; <text>` — iTerm2/WezTerm/Ghostty notifications. The
//!   ConEmu progress form (`OSC 9 ; 4 ; …`) is dropped: agents emit it
//!   once a second while they work and it means "busy", not "yours".
//! * `OSC 777 ; notify ; <title> ; <body>` — urxvt notifications.
//!
//! The scanner is incremental: an escape sequence can be split across
//! read chunks, so partial state survives between calls. It is fed the
//! raw bytes beside the terminal parser — it never consumes anything.

/// Longest OSC payload collected before the sequence is abandoned (a
/// stuck `ESC ]` must not grow the buffer without bound).
const OSC_MAX: usize = 1024;

/// One "the user is needed" signal decoded from the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attention {
    /// Notification title; `None` when the source carries none.
    pub title: Option<String>,
    /// Notification body; empty for a bare bell.
    pub body: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Ground,
    /// Saw `ESC`; `]` starts an OSC string, anything else is ignored.
    Escape,
    /// Inside an OSC string, collecting its payload.
    Osc,
    /// Saw `ESC` inside an OSC string: `\` ends it (ST terminator).
    OscEscape,
}

#[derive(Default)]
pub struct Scanner {
    state: State,
    osc: Vec<u8>,
    /// The current OSC exceeded [`OSC_MAX`]; collect nothing more until
    /// its terminator arrives.
    overflow: bool,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan one read chunk, appending every signal it completes.
    pub fn scan(&mut self, bytes: &[u8], out: &mut Vec<Attention>) {
        for &byte in bytes {
            match self.state {
                State::Ground => match byte {
                    0x07 => out.push(Attention {
                        title: None,
                        body: String::new(),
                    }),
                    0x1b => self.state = State::Escape,
                    _ => {}
                },
                State::Escape => {
                    self.state = if byte == b']' {
                        self.osc.clear();
                        self.overflow = false;
                        State::Osc
                    } else {
                        State::Ground
                    };
                }
                State::Osc => match byte {
                    0x07 => self.finish_osc(out),
                    0x1b => self.state = State::OscEscape,
                    _ if self.overflow => {}
                    _ if self.osc.len() < OSC_MAX => self.osc.push(byte),
                    _ => self.overflow = true,
                },
                State::OscEscape => {
                    // ST (`ESC \`) ends the string; a stray ESC inside is
                    // dropped and collection continues.
                    if byte == b'\\' {
                        self.finish_osc(out);
                    } else {
                        self.state = State::Osc;
                    }
                }
            }
        }
    }

    fn finish_osc(&mut self, out: &mut Vec<Attention>) {
        if !self.overflow
            && let Some(attention) = parse_osc(&self.osc)
        {
            out.push(attention);
        }
        self.osc.clear();
        self.overflow = false;
        self.state = State::Ground;
    }
}

/// Decode an OSC payload (without the `ESC ]` prefix and terminator).
fn parse_osc(payload: &[u8]) -> Option<Attention> {
    let text = String::from_utf8_lossy(payload);
    let (verb, rest) = text.split_once(';')?;
    match verb {
        "9" => {
            // ConEmu progress reports (`9;4;<state>`, `9;<0..3>`), also
            // used by `omp` while a turn is running.
            if rest == "4" || rest.starts_with("4;") {
                return None;
            }
            Some(Attention {
                title: None,
                body: rest.trim().into(),
            })
        }
        "777" => {
            let rest = rest.strip_prefix("notify;")?;
            let (title, body) = rest.split_once(';').unwrap_or(("", rest));
            Some(Attention {
                title: (!title.trim().is_empty()).then(|| title.trim().into()),
                body: body.trim().into(),
            })
        }
        // Window titles (`0/1/2`), colors, clipboard, hyperlinks — not ours.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<Attention> {
        let mut scanner = Scanner::new();
        let mut out = Vec::new();
        for chunk in chunks {
            scanner.scan(chunk, &mut out);
        }
        out
    }

    #[test]
    fn bell_reports_attention() {
        assert_eq!(
            scan(&[b"done\x07"]),
            vec![Attention {
                title: None,
                body: String::new()
            }]
        );
    }

    #[test]
    fn osc9_body_uses_both_terminators() {
        let bel = scan(&[b"\x1b]9;turn complete\x07"]);
        assert_eq!(bel.len(), 1);
        assert_eq!(bel[0].body, "turn complete");
        assert_eq!(bel[0].title, None);
        // iTerm2 style ST terminator, split across reads.
        let st = scan(&[b"\x1b]9;", b"build failed\x1b", b"\\"]);
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].body, "build failed");
    }

    #[test]
    fn osc9_progress_is_not_attention() {
        // omp's running-state progress indicator.
        assert!(scan(&[b"\x1b]9;4;3\x07"]).is_empty());
        assert!(scan(&[b"\x1b]9;4;0;\x07"]).is_empty());
    }

    #[test]
    fn osc777_carries_title_and_body() {
        let out = scan(&[b"\x1b]777;notify;make;done in 4s\x07"]);
        assert_eq!(
            out,
            vec![Attention {
                title: Some("make".into()),
                body: "done in 4s".into()
            }]
        );
        // No title: the whole remainder is the body.
        let out = scan(&[b"\x1b]777;notify;;just a body\x07"]);
        assert_eq!(out[0].title, None);
        assert_eq!(out[0].body, "just a body");
    }

    #[test]
    fn other_sequences_are_ignored() {
        // Window title, clipboard, hyperlink, colors.
        assert!(scan(&[b"\x1b]0;zsh\x07"]).is_empty());
        assert!(scan(&[b"\x1b]2;\x07"]).is_empty());
        assert!(scan(&[b"\x1b]52;c;Zm9v\x07"]).is_empty());
        assert!(scan(&[b"\x1b]8;;http://x\x07link\x1b]8;;\x07"]).is_empty());
        assert!(scan(&[b"\x1b[31mred\x1b[0m"]).is_empty());
    }

    #[test]
    fn unterminated_osc_does_not_leak_into_later_bytes() {
        // A truncated OSC, then a bell: only the bell counts.
        let out = scan(&[b"\x1b]9;never closed", b"\x07\x07"]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].body, "never closed");
        assert_eq!(out[1].body, "");
    }
}
