//! The app's key bindings in one place: the platform chords the terminal
//! surface shares with the shell it runs, the accelerator labels
//! user-facing text prints, the binding table `AppView::new` installs —
//! which the routing tests below pin, precedence included — and the
//! rebinding surface the settings window edits ([`Command`]).
//!
//! Every rebindable shortcut is one [`Command`]: a stable id (the key it
//! takes in `settings.json`'s `keys` map), the settings row it renders
//! as, and the builtin chords that dispatch it. An override replaces
//! *all* of a command's builtin chords with the one chord the user
//! pressed, and does it by **appending** to the keymap, never by
//! rebuilding it: gpui has no "remove binding", but an [`Unbind`] binding
//! added *after* a chord hides the earlier bindings for that action at
//! that chord (`Keymap::bindings_for_input` walks the bindings
//! newest-first and drops what a higher-precedence `Unbind` names). So an
//! override is a small layer — the new chord, plus an `Unbind` on every
//! chord it replaces, including the chords of the *previous* layer, which
//! is why the applied one is remembered in [`AppliedKeys`]. Clearing the
//! whole keymap instead is not an option: gpui-component's own bindings
//! (the inputs' arrows, `Root`'s tab cycling, the popup menus) live in the
//! same map and are registered by its `init`.

use std::collections::BTreeMap;

use super::*;
use crate::config::Config;

// ── command ids ───────────────────────────────────────────────────────
//
// The ids are the keys of `settings.json`'s `keys` map, the identity the
// settings page edits and the one `accel_hint` is called with. They live
// in constants so the table, the settings rows and the tabs, tooltips and
// menus that print a chord all name the same string — a typo is then a
// compile error instead of a hint that silently prints nothing. (The nine
// session digits are spelled out in the table: nothing outside it looks
// them up by name.)

/// Spawn the default launcher in the active project.
pub(crate) const NEW_SESSION: &str = "new_session";
/// Folder picker: add a project.
pub(crate) const ADD_PROJECT: &str = "add_project";
/// Open (or raise) the settings window.
pub(crate) const OPEN_SETTINGS: &str = "open_settings";
/// Save the workspace and quit.
pub(crate) const QUIT: &str = "quit";
/// Close the current session's row.
pub(crate) const CLOSE_SESSION: &str = "close_session";
/// Show/hide the project and session list.
pub(crate) const TOGGLE_SESSIONS: &str = "toggle_sessions";
/// Show/hide the diff pane.
pub(crate) const TOGGLE_DIFF: &str = "toggle_diff";
/// Show/hide the file tree under the project list.
pub(crate) const TOGGLE_DIFF_TREE: &str = "toggle_diff_tree";
/// Step the pane through file → diff → rendered Markdown.
pub(crate) const TOGGLE_VIEW_MODE: &str = "toggle_view_mode";
/// Zoom the terminal font size up.
pub(crate) const FONT_LARGER: &str = "font_larger";
/// Zoom the terminal font size down.
pub(crate) const FONT_SMALLER: &str = "font_smaller";
/// The quick open over the working tree.
pub(crate) const FILE_SEARCH: &str = "file_search";
/// The find bar of whichever pane holds focus.
pub(crate) const FIND: &str = "find";
/// Copy the selected file's path.
pub(crate) const COPY_PATH: &str = "copy_path";
/// Copy the selected file's contents.
pub(crate) const COPY_CONTENTS: &str = "copy_contents";

/// Copy / paste / find, the three chords the terminal surface shares
/// with the shell it runs. macOS keeps the ⌘ chords; elsewhere they
/// move into the ⌃⇧ space, because ⌃C is the shell's SIGINT and ⌃V / ⌃F
/// belong to readline — a terminal that swallowed those would be
/// unusable. ⌃⇧* is exactly the space Linux terminals already reserve
/// for their own actions.
pub(crate) const COPY_ACCEL: &str = if cfg!(target_os = "macos") {
    "cmd-c"
} else {
    "ctrl-shift-c"
};
pub(crate) const PASTE_ACCEL: &str = if cfg!(target_os = "macos") {
    "cmd-v"
} else {
    "ctrl-shift-v"
};
pub(crate) const FIND_ACCEL: &str = if cfg!(target_os = "macos") {
    "cmd-f"
} else {
    "ctrl-shift-f"
};
/// The two file-copy commands behind the tree's and the pane's context
/// menus. ⌥⌘C is the copy-pathname chord Finder and VS Code use; the
/// contents take its shifted variant. Both sit in the ⌃⌥ space on
/// Linux, which no shell reads (⌥C alone is readline's capitalize-word,
/// ⌃⌥C is free) — so a menu that shows them shows a chord that works.
pub(crate) const COPY_PATH_ACCEL: &str = if cfg!(target_os = "macos") {
    "cmd-alt-c"
} else {
    "ctrl-alt-c"
};
pub(crate) const COPY_CONTENTS_ACCEL: &str = if cfg!(target_os = "macos") {
    "cmd-alt-shift-c"
} else {
    "ctrl-alt-shift-c"
};
/// The quick open's chord: ⌘P on macOS, and on Linux another chord the
/// shell gives up (⌃P is readline's previous-history) — the same trade
/// ⌃R already makes, for a search over every file in the project.
pub(crate) const FILE_SEARCH_ACCEL: &str = if cfg!(target_os = "macos") {
    "cmd-p"
} else {
    "ctrl-p"
};

// ── the table ─────────────────────────────────────────────────────────

/// One keystroke the app binds: the chord, the key context it needs, and
/// the action it dispatches.
pub(crate) struct Chord {
    /// gpui's syntax. `secondary-` is ⌘ on macOS and ⌃ elsewhere, which
    /// is why the platform constants above exist at all.
    pub chord: &'static str,
    /// `None` = anywhere. A named context makes the binding deeper (it
    /// wins where that context is on the dispatch path); a `!`-prefixed
    /// one is gpui's *negative* context, for the chords that must lose to
    /// a deeper binding of the same keystroke — see the comments on
    /// [`key_bindings`].
    pub context: Option<&'static str>,
    pub action: fn() -> Box<dyn Action>,
}

/// A shortcut the settings page offers for rebinding, and the settings
/// row it renders as.
pub(crate) struct Command {
    /// Key in `settings.json`'s `keys` map. Stable — an override is
    /// stored and looked up by it, never by chord.
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Settings group the row lands in, in table order.
    pub group: &'static str,
    /// The chords the builtin table binds: the first is what the settings
    /// row prints, and one override replaces every one of them.
    pub chords: &'static [Chord],
}

/// A chord the app keeps for itself: the keys a terminal session cannot
/// give up (the shell's own tab and readline chords), the bars' stepping,
/// and the settings window's escape. Listed on the settings page as a
/// read-only row — the answer to "why can't I rebind ⌘C?" — and counted
/// as taken when a capture lands on one.
pub(crate) struct Reserved {
    pub label: &'static str,
    pub description: &'static str,
    pub chord: Chord,
}

macro_rules! chord {
    ($chord:expr, $context:expr, $action:path) => {
        Chord {
            chord: $chord,
            context: $context,
            action: || Box::new($action),
        }
    };
}

macro_rules! command {
    ($id:expr, $label:literal, $description:literal, $group:literal, [$($chord:expr),* $(,)?]) => {
        Command {
            id: $id,
            label: $label,
            description: $description,
            group: $group,
            chords: &[$($chord),*],
        }
    };
}

/// The rebindable shortcuts, grouped as the settings page lists them.
/// Registration order is this order (the settings page reads it too), and
/// none of these chords is claimed by another — [`chord_taken_by`] is
/// what keeps a capture from breaking that.
pub(crate) const COMMANDS: &[Command] = &[
    // ── Application ──
    command!(
        NEW_SESSION,
        "New session",
        "Spawn the default launcher in the active project.",
        "Application",
        [chord!("secondary-n", None, NewSession)]
    ),
    command!(
        ADD_PROJECT,
        "Add project",
        "Pick a folder and add it as a project.",
        "Application",
        [chord!("secondary-o", None, AddProject)]
    ),
    command!(
        OPEN_SETTINGS,
        "Settings",
        "Open the settings window, or bring it forward.",
        "Application",
        [chord!("secondary-,", None, OpenSettings)]
    ),
    command!(
        QUIT,
        "Quit",
        "Save the workspace and quit — sessions that are still running are asked about first.",
        "Application",
        [chord!("secondary-q", None, Quit)]
    ),
    // ── Layout ──
    command!(
        TOGGLE_SESSIONS,
        "Toggle sidebar",
        "Show or hide the project and session list.",
        "Layout",
        [chord!("secondary-b", None, ToggleSessions)]
    ),
    command!(
        TOGGLE_DIFF,
        "Toggle changes pane",
        "Show or hide the diff pane (clicking a file opens it too).",
        "Layout",
        [chord!("secondary-r", None, ToggleDiff)]
    ),
    command!(
        TOGGLE_DIFF_TREE,
        "Toggle file tree",
        "Show or hide the file tree under the project list.",
        "Layout",
        [chord!("secondary-t", None, ToggleDiffTree)]
    ),
    command!(
        TOGGLE_VIEW_MODE,
        "Cycle the file view",
        "Step the right pane through file → diff → rendered Markdown (Markdown files only).",
        "Layout",
        [chord!("secondary-shift-m", None, ToggleViewMode)]
    ),
    command!(
        FONT_LARGER,
        "Larger terminal text",
        "Zoom the terminal font size up; the size is persisted.",
        "Layout",
        [
            chord!("secondary-=", None, FontLarger),
            chord!("secondary-+", None, FontLarger),
        ]
    ),
    command!(
        FONT_SMALLER,
        "Smaller terminal text",
        "Zoom the terminal font size down; the size is persisted.",
        "Layout",
        [
            chord!("secondary--", None, FontSmaller),
            chord!("secondary-_", None, FontSmaller),
        ]
    ),
    // ── Files ──
    command!(
        FILE_SEARCH,
        "Quick open",
        "Search every path in the working tree: type a fragment like `difpan`.",
        "Files",
        [chord!(FILE_SEARCH_ACCEL, None, FileSearch)]
    ),
    command!(
        FIND,
        "Find in the pane",
        "Open the find bar of the pane holding focus — the terminal's or the diff's.",
        "Files",
        [
            chord!(FIND_ACCEL, Some("Terminal"), TermSearch),
            chord!(FIND_ACCEL, Some("!Terminal"), DiffSearch),
        ]
    ),
    command!(
        COPY_PATH,
        "Copy file path",
        "Copy the selected file's path (the tree's and the pane's menu both run it).",
        "Files",
        [chord!(COPY_PATH_ACCEL, None, CopyFilePath)]
    ),
    command!(
        COPY_CONTENTS,
        "Copy file contents",
        "Copy the selected file's text.",
        "Files",
        [chord!(COPY_CONTENTS_ACCEL, None, CopyFileContents)]
    ),
    // ── Sessions ──
    command!(
        CLOSE_SESSION,
        "Close session",
        "Close the current session's row; a live agent is asked about first, and its \
         conversation id is kept so the row can be resumed.",
        "Sessions",
        [chord!("secondary-w", None, CloseSession)]
    ),
    // Prefixed so bare digits keep reaching the PTY.
    command!(
        "select_session_1",
        "Select session 1",
        "Focus the first session of the current project.",
        "Sessions",
        [chord!("secondary-1", None, SelectSession1)]
    ),
    command!(
        "select_session_2",
        "Select session 2",
        "Focus the second session of the current project.",
        "Sessions",
        [chord!("secondary-2", None, SelectSession2)]
    ),
    command!(
        "select_session_3",
        "Select session 3",
        "Focus the third session of the current project.",
        "Sessions",
        [chord!("secondary-3", None, SelectSession3)]
    ),
    command!(
        "select_session_4",
        "Select session 4",
        "Focus the fourth session of the current project.",
        "Sessions",
        [chord!("secondary-4", None, SelectSession4)]
    ),
    command!(
        "select_session_5",
        "Select session 5",
        "Focus the fifth session of the current project.",
        "Sessions",
        [chord!("secondary-5", None, SelectSession5)]
    ),
    command!(
        "select_session_6",
        "Select session 6",
        "Focus the sixth session of the current project.",
        "Sessions",
        [chord!("secondary-6", None, SelectSession6)]
    ),
    command!(
        "select_session_7",
        "Select session 7",
        "Focus the seventh session of the current project.",
        "Sessions",
        [chord!("secondary-7", None, SelectSession7)]
    ),
    command!(
        "select_session_8",
        "Select session 8",
        "Focus the eighth session of the current project.",
        "Sessions",
        [chord!("secondary-8", None, SelectSession8)]
    ),
    command!(
        "select_session_9",
        "Select session 9",
        "Focus the ninth session of the current project.",
        "Sessions",
        [chord!("secondary-9", None, SelectSession9)]
    ),
];

/// The chords no command owns. Registered after the commands (a tie in
/// precedence goes to the later binding, and the pairs below rely on
/// their *depth* rule, not on order — see [`key_bindings`]).
pub(crate) const RESERVED: &[Reserved] = &[
    Reserved {
        label: "Tab and Shift-Tab",
        description: "Sent to the shell: the terminal surface outranks Root's focus cycling.",
        chord: chord!("tab", Some("Terminal"), TermTab),
    },
    Reserved {
        label: "Shift-Tab",
        description: "Sent to the shell, like Tab.",
        chord: chord!("shift-tab", Some("Terminal"), TermBacktab),
    },
    Reserved {
        label: "Paste into the terminal",
        description: "Pastes the clipboard into the PTY; the shell owns the unshifted chord.",
        chord: chord!(PASTE_ACCEL, Some("Terminal"), TermPaste),
    },
    Reserved {
        label: "Copy from the terminal",
        description: "Grabs the mouse selection; with nothing selected the chord goes to the shell.",
        chord: chord!(COPY_ACCEL, Some("Terminal"), TermCopy),
    },
    Reserved {
        label: "Copy a text selection",
        description: "Copies the selection in the diff pane or a rendered document.",
        chord: chord!(COPY_ACCEL, Some("!Terminal"), input::Copy),
    },
    Reserved {
        label: "Next match",
        description: "Steps the diff pane's find bar.",
        chord: chord!("secondary-g", Some("DiffSearch"), DiffSearchNext),
    },
    Reserved {
        label: "Previous match",
        description: "Steps the diff pane's find bar.",
        chord: chord!("secondary-shift-g", Some("DiffSearch"), DiffSearchPrev),
    },
    Reserved {
        label: "Next hit",
        description: "Steps the quick open's list.",
        chord: chord!("secondary-g", Some("FileSearch"), FileSearchNext),
    },
    Reserved {
        label: "Previous hit",
        description: "Steps the quick open's list.",
        chord: chord!("secondary-shift-g", Some("FileSearch"), FileSearchPrev),
    },
    Reserved {
        label: "Down",
        description: "Steps the quick open; the arrow belongs to its field elsewhere.",
        chord: chord!("down", Some("FileSearch"), FileSearchNext),
    },
    Reserved {
        label: "Up",
        description: "Steps the quick open; the arrow belongs to its field elsewhere.",
        chord: chord!("up", Some("FileSearch"), FileSearchPrev),
    },
    Reserved {
        label: "Next match in the terminal",
        description: "Steps the terminal's find bar.",
        chord: chord!("secondary-g", Some("TerminalSearch"), TermSearchNext),
    },
    Reserved {
        label: "Previous match in the terminal",
        description: "Steps the terminal's find bar.",
        chord: chord!("secondary-shift-g", Some("TerminalSearch"), TermSearchPrev),
    },
    Reserved {
        label: "Close the settings window",
        description: "The window's own Escape.",
        chord: chord!("escape", Some("SettingsWindow"), CloseSettings),
    },
    Reserved {
        label: "Close the settings window",
        description: "The window's own chord, deeper than the app-wide close.",
        chord: chord!("secondary-w", Some("SettingsWindow"), CloseSettings),
    },
];

/// One key binding from a boxed action, the way the table means it: the
/// key context parsed out of the entry, no key equivalents, no action
/// input. `KeyBinding::new` would want a *sized* `Action`, and the table
/// carries `fn() -> Box<dyn Action>` so a single field can hold any of
/// them (and `Unbind`, which names an action as a string).
///
/// `None` — an unparseable chord or a context typo — is a table bug at
/// the call sites that pass a constant, and an ignored override where the
/// chord came from `settings.json` (`validate_keys` is what reports it).
fn binding(chord: &str, action: Box<dyn Action>, context: Option<&str>) -> Option<KeyBinding> {
    let context = match context {
        Some(context) => Some(KeyBindingContextPredicate::parse(context).ok()?.into()),
        None => None,
    };
    KeyBinding::load(chord, action, context, false, None, &DummyKeyboardMapper).ok()
}

/// The full key binding table, in one place so `bind_keys` and the
/// routing regression test share it. Bindings for the same keystroke
/// form a fallback chain: gpui dispatches them in precedence order
/// (deepest matching context slice first, ties to the later entry)
/// until one handler stops propagation — see gpui's
/// `bindings_for_input` / `dispatch_key`.
///
/// Every app shortcut is a `secondary-` chord: ⌘ on macOS, ⌃ elsewhere.
/// A chord a binding claims never reaches the PTY — gpui's bubble-phase
/// action dispatch stops propagation before the terminal's key listener
/// runs — so on Linux these override the shell's own readline keys while
/// the terminal holds focus (`⌃R` toggles the changes pane, not reverse
/// search); the shell keeps everything the app leaves unbound.
///
/// The two `!Terminal` rows are the whole trick of the shared chords:
/// `COPY_ACCEL` and `FIND_ACCEL` each open the surface the *focus* is in,
/// and a predicate-less binding cannot do that — it always scores the
/// full context stack, so it *ties* the named context at the focused
/// element and then wins the later-binding tiebreak (that is how the
/// terminal's ⌘C dispatched the pane's copy first, and why the terminal's
/// right-click menu — which hints an item from the action it carries and
/// accepts only the chord's *winner* — printed no chord for `TermCopy` at
/// all). `!Terminal` (true only while the terminal is nowhere on the
/// dispatch path) matches one slice *shallower* than `Terminal` itself,
/// so the positive binding always wins by exactly one level whenever the
/// terminal surface or its find bar holds focus, and the diff-side
/// binding wins anywhere else ("Terminal" absent from every slice).
/// While a search bar's input already holds focus its own action
/// refocuses it with the query selected, matching platform find bars.
pub(super) fn key_bindings() -> Vec<KeyBinding> {
    COMMANDS
        .iter()
        .flat_map(|command| command.chords)
        .chain(RESERVED.iter().map(|reserved| &reserved.chord))
        .map(|chord| {
            binding(chord.chord, (chord.action)(), chord.context)
                .unwrap_or_else(|| panic!("the table's \"{}\" is not bindable", chord.chord))
        })
        .collect()
}

// ── reading a command's shortcut ──────────────────────────────────────

/// Print a stored chord the way the platform writes shortcuts — `⌘⇧N` on
/// macOS, `Ctrl+Shift+N` elsewhere. gpui-component's own rendering of a
/// keystroke, so menus, tooltips and the settings rows agree on one
/// spelling. A chord that does not parse prints as stored: a hand-edited
/// `settings.json` should show what is in it rather than nothing.
pub(crate) fn chord_label(chord: &str) -> String {
    match Keystroke::parse(chord) {
        Ok(stroke) => gpui_kit::component::kbd::Kbd::format(&stroke),
        Err(_) => chord.to_string(),
    }
}

/// Parse a stored chord into a bindable keystroke: gpui's syntax, plus
/// the rule the recorder enforces — one of ⌃, ⌥ or ⌘/Super. A
/// modifier-less chord would be swallowed from every text field and every
/// PTY, which is why the builtin table has none either.
pub(crate) fn parse_chord(chord: &str) -> Result<Keystroke, String> {
    let stroke = Keystroke::parse(chord).map_err(|_| format!("\"{chord}\" is not a keystroke"))?;
    if !has_command_modifier(&stroke) {
        return Err(format!("\"{chord}\" needs Ctrl, Alt or ⌘/Super"));
    }
    Ok(stroke)
}

/// ⌃, ⌥ or ⌘/Super — the modifiers an OS reserves for commands. Shift
/// alone only produces capitals, so it does not count.
fn has_command_modifier(stroke: &Keystroke) -> bool {
    stroke.modifiers.control || stroke.modifiers.alt || stroke.modifiers.platform
}

/// A keystroke that is only a modifier press. gpui reports the modifier
/// keys themselves; the chord is not complete until a real key lands
/// (⌘ before N).
fn is_modifier_press(stroke: &Keystroke) -> bool {
    matches!(
        stroke.key.as_str(),
        "shift"
            | "control"
            | "ctrl"
            | "alt"
            | "option"
            | "platform"
            | "cmd"
            | "super"
            | "win"
            | "meta"
            | "function"
            | "fn"
    )
}

/// The overrides this config actually puts in force: known commands only,
/// chords that parse and carry a command modifier, and only when the
/// chord differs from the builtin one (so a chord the user picked back to
/// its default does not stay in the map — and, in particular, never gets
/// bound *and* unbound in the same layer). Everything filtered out here
/// is reported by [`validate_keys`].
fn overrides(cfg: &Config) -> BTreeMap<String, String> {
    cfg.keys
        .iter()
        .filter(|(id, raw)| {
            COMMANDS
                .iter()
                .find(|command| command.id == id.as_str())
                .is_some_and(|command| {
                    parse_chord(raw).is_ok() && !is_builtin(command, raw)
                })
        })
        .map(|(id, raw)| (id.clone(), raw.clone()))
        .collect()
}

/// Whether two stored chords are the same keystroke (compared parsed, not
/// as strings: `secondary-n` and `super-n` are the same chord on Linux).
fn same_chord(a: &str, b: &str) -> bool {
    match (Keystroke::parse(a), Keystroke::parse(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Whether `chord` is one of the command's builtin chords.
fn is_builtin(command: &Command, chord: &str) -> bool {
    command
        .chords
        .iter()
        .any(|c| same_chord(c.chord, chord))
}

/// A command's shortcuts as the settings row shows them. The chord the
/// command *runs* on is what [`accel_hint`] computes — the same override
/// filter, without the display-only states below.
pub(crate) struct Shortcut {
    /// What the row prints: the override, or every builtin chord.
    pub text: String,
    /// The stored override cannot be used — the row shows the stored text
    /// and this reason, and offers a reset.
    pub broken: Option<String>,
    /// An override is in force and differs from the builtin chords.
    pub custom: bool,
}

/// Read one command's state off the config.
pub(crate) fn shortcut(command: &Command, cfg: &Config) -> Shortcut {
    let builtin = || {
        command
            .chords
            .iter()
            .map(|c| chord_label(c.chord))
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let Some(raw) = cfg.keys.get(command.id) else {
        return Shortcut {
            text: builtin(),
            broken: None,
            custom: false,
        };
    };
    match parse_chord(raw) {
        Err(reason) => Shortcut {
            text: raw.clone(),
            broken: Some(reason),
            custom: true,
        },
        Ok(_) if is_builtin(command, raw) => Shortcut {
            text: builtin(),
            broken: None,
            custom: false,
        },
        Ok(_) => Shortcut {
            text: chord_label(raw),
            broken: None,
            custom: true,
        },
    }
}

/// Shortcut label for user-facing text (`⌘N` on macOS, `Ctrl+N`
/// elsewhere) — the command's *current* chord, so a rebind moves every
/// tooltip and menu hint with it. An unknown id prints nothing rather
/// than a wrong chord.
pub(crate) fn accel_hint(id: &str, cx: &App) -> String {
    let cfg = cx.try_global::<crate::config::Config>();
    let Some(command) = COMMANDS.iter().find(|command| command.id == id) else {
        debug_assert!(false, "accel_hint({id}) is not a command id");
        return String::new();
    };
    match cfg.and_then(|cfg| overrides(cfg).get(id).cloned()) {
        Some(chord) => chord_label(&chord),
        None => chord_label(command.chords[0].chord),
    }
}

/// The label of whoever already owns `stroke`, for a capture that landed
/// on a taken chord, or `None` when it is free. `editing` is the command
/// being rebound: its own chords are not a conflict with itself.
pub(crate) fn chord_taken_by(stroke: &Keystroke, cfg: &Config, editing: &str) -> Option<String> {
    let custom = overrides(cfg);
    let edited = COMMANDS.iter().find(|command| command.id == editing);
    for command in COMMANDS.iter().filter(|command| command.id != editing) {
        let taken = match custom.get(command.id) {
            Some(chord) => Keystroke::parse(chord).is_ok_and(|c| c == *stroke),
            None => command
                .chords
                .iter()
                .any(|c| Keystroke::parse(c.chord).is_ok_and(|builtin| builtin == *stroke)),
        };
        if taken {
            return Some(command.label.to_string());
        }
    }
    for reserved in RESERVED {
        // A reserved chord the command already *has* — close-session's ⌘W
        // is the settings window's too — is not a conflict with it: the
        // deeper context wins today, and picking it back is a no-op.
        if edited.is_some_and(|command| is_builtin(command, reserved.chord.chord)) {
            continue;
        }
        if same_chord(reserved.chord.chord, &stroke.unparse()) {
            return Some(reserved.label.to_string());
        }
    }
    None
}

/// What a keystroke inside the settings recorder means — the whole rule,
/// so the page only has to show the answer.
pub(crate) enum Captured {
    /// A modifier press: the chord is still incomplete.
    Waiting,
    /// Escape: the user gave up.
    Cancelled,
    /// Refused, with the reason to print under the row.
    Refused(String),
    /// The chord to bind, in gpui's syntax.
    Chord(String),
}

/// Decide what pressing `stroke` while rebinding `id` means.
pub(crate) fn captured(stroke: &Keystroke, id: &str, cfg: &Config) -> Captured {
    if is_modifier_press(stroke) {
        return Captured::Waiting;
    }
    if stroke.key == "escape" && !has_command_modifier(stroke) {
        return Captured::Cancelled;
    }
    if !has_command_modifier(stroke) {
        return Captured::Refused(
            "A shortcut needs Ctrl, Alt or ⌘/Super — a bare key would be swallowed from \
             every field and every session."
                .to_string(),
        );
    }
    if let Some(other) = chord_taken_by(stroke, cfg, id) {
        return Captured::Refused(format!(
            "{} is taken by \"{other}\"",
            chord_label(&stroke.unparse())
        ));
    }
    Captured::Chord(stroke.unparse())
}

/// Problems worth telling the user about in the `keys` map of
/// `settings.json`, for the startup dialog: ids this build does not know,
/// chords it cannot bind, and chords that collide with another shortcut
/// (a hand-edit; the recorder refuses those itself).
pub(crate) fn validate_keys(cfg: &Config) -> Vec<String> {
    let mut warnings = Vec::new();
    let file = crate::config::settings_path().display().to_string();
    for (id, raw) in &cfg.keys {
        let Some(command) = COMMANDS.iter().find(|command| command.id == id.as_str()) else {
            warnings.push(format!("{file}: \"{id}\" is not a shortcut of this app"));
            continue;
        };
        match parse_chord(raw) {
            Err(reason) => warnings.push(format!(
                "{file}: \"{}\" for \"{}\": {reason}",
                command.label, id
            )),
            Ok(stroke) => {
                if let Some(other) = chord_taken_by(&stroke, cfg, id) {
                    warnings.push(format!(
                        "{file}: \"{}\" is also \"{other}\"'s shortcut — one of them will not \
                         reach its command",
                        chord_label(raw)
                    ));
                }
            }
        }
    }
    warnings
}

// ── installing ────────────────────────────────────────────────────────

/// Keys applied by the last [`apply_overrides`]: the overrides
/// themselves (a no-op re-apply when they did not change) and the
/// `(chord, action)` pairs they added, so the next layer can unbind them.
#[derive(Default)]
struct AppliedKeys {
    overrides: BTreeMap<String, String>,
    added: Vec<(String, String)>,
}

impl Global for AppliedKeys {}

/// Set the config's keyboard overrides on top of the builtin table.
///
/// Appended, never rebuilt: the layer binds each command on the chord it
/// runs on now and *unbinds* the chords it is moving off — the command's
/// builtin ones, and whatever the previous layer bound (which is how a
/// second rebind takes the first one out). Called on startup and after
/// every settings edit; a layer whose overrides did not change is
/// skipped, so the map does not grow on every unrelated settings tweak.
///
/// A command a layer has touched — it is overridden now, or a previous
/// layer bound or retired one of its chords — is re-stated on the chord it
/// runs on now, and only such a command is: an `Unbind` is permanent for
/// the bindings under it, so a reset can only put a retired chord back by
/// binding it *again*, later in the list. (Re-stating every command would
/// work too and leaves a duplicate in the chain for nothing; the builtin
/// table already carries the untouched ones.)
pub(crate) fn apply_overrides(cx: &mut App) {
    let cfg = cx
        .try_global::<crate::config::Config>()
        .cloned()
        .unwrap_or_default();
    let overrides = overrides(&cfg);
    if cx
        .try_global::<AppliedKeys>()
        .is_some_and(|applied| applied.overrides == overrides)
    {
        return;
    }

    let mut layer = Vec::new();
    let previous = cx
        .try_global::<AppliedKeys>()
        .map(|applied| applied.added.clone())
        .unwrap_or_default();
    for (chord, action) in &previous {
        if let Some(retire) = binding(chord, Box::new(Unbind(action.clone().into())), None) {
            layer.push(retire);
        }
    }

    let mut added = Vec::new();
    for command in COMMANDS {
        let custom = overrides.get(command.id).map(String::as_str);
        // Only a command a layer has actually touched (or is touching) is
        // re-stated: an untouched one already has its binding in the
        // builtin table, and re-stating it would leave a duplicate in the
        // chain for nothing.
        let restate = custom.is_some()
            || command.chords.iter().any(|builtin| {
                let name = (builtin.action)().name();
                previous
                    .iter()
                    .any(|(_, retired)| retired.as_str() == name)
            });
        if !restate {
            continue;
        }
        for builtin in command.chords {
            let action = (builtin.action)();
            let name = action.name().to_string();
            let chord = custom.unwrap_or(builtin.chord);
            // The chord this command runs on now, re-stated so that an
            // earlier layer's `Unbind` of it cannot outlive that layer.
            if let Some(rebind) = binding(chord, action, builtin.context) {
                layer.push(rebind);
                added.push((chord.to_string(), name.clone()));
            }
            // …and the builtin chord it left, if it moved off one.
            if custom.is_some()
                && let Some(clear) =
                    binding(builtin.chord, Box::new(Unbind(name.clone().into())), None)
            {
                layer.push(clear);
            }
        }
    }
    cx.bind_keys(layer);

    let applied = AppliedKeys { overrides, added };
    if cx.try_global::<AppliedKeys>().is_none() {
        cx.set_global(applied);
    } else {
        *cx.global_mut::<AppliedKeys>() = applied;
    }
}

/// Set when the builtin table has been bound; see [`install`].
struct Installed;

impl Global for Installed {}

/// Install the app's bindings — the builtin table, then the config's
/// overrides on top of it.
///
/// [`AppView::new`] calls this (a window can be re-created from the dock),
/// and `main` calls it once before building the app menu, which resolves
/// its shortcut hints from the keymap. The table itself is bound exactly
/// once per process: a re-added builtin binding lands *after* the override
/// layer and would out-rank it (a later entry wins the tie), silently
/// resurrecting a chord the user rebound.
pub(crate) fn install(cx: &mut App) {
    if cx.try_global::<Installed>().is_none() {
        cx.set_global(Installed);
        cx.bind_keys(key_bindings());
    }
    apply_overrides(cx);
}

#[cfg(test)]
mod tests {
    use super::{
        COMMANDS, Captured, FILE_SEARCH_ACCEL, FIND_ACCEL, RESERVED, chord_label, chord_taken_by,
        captured, key_bindings, overrides, validate_keys,
    };
    use crate::config::Config;
    use gpui_kit::{KeyBinding, KeyContext, Keymap, Keystroke, Unbind};

    /// The fallback chain gpui would dispatch for `keystroke`, in
    /// precedence order, given a context stack (bottom → top, as the
    /// dispatch tree builds it).
    fn chain_of(keymap: &Keymap, keystroke: &str, stack: &[&str]) -> Vec<String> {
        let contexts = stack
            .iter()
            .map(|c| KeyContext::parse(c).unwrap())
            .collect::<Vec<_>>();
        let keystrokes = vec![Keystroke::parse(keystroke).unwrap()];
        let (bindings, _) = keymap.bindings_for_input(&keystrokes, &contexts);
        bindings
            .iter()
            .map(|b| b.action().name().to_string())
            .collect()
    }

    /// The same, against the builtin table.
    fn chain(keystroke: &str, stack: &[&str]) -> Vec<String> {
        chain_of(&Keymap::new(key_bindings()), keystroke, stack)
    }

    /// Highest-precedence action gpui would dispatch for `keystroke`.
    fn winner(keystroke: &str, stack: &[&str]) -> String {
        chain(keystroke, stack).first().cloned().unwrap_or_default()
    }

    /// The find chord must open the search bar of whichever pane holds
    /// focus. Regression: the diff binding used a predicate-less
    /// context, which ties any named context on depth and wins the
    /// later-binding tiebreak — so the chord in the terminal opened the
    /// diff pane's bar.
    #[test]
    fn find_routes_by_focus() {
        // Terminal surface focused.
        assert_eq!(winner(FIND_ACCEL, &["Root", "Terminal"]), "ddu::TermSearch");
        // Terminal find bar's input focused (its own context on top
        // of the surface's).
        assert_eq!(
            winner(FIND_ACCEL, &["Root", "Terminal", "TerminalSearch"]),
            "ddu::TermSearch"
        );
        // Diff find bar's input focused.
        assert_eq!(winner(FIND_ACCEL, &["Root", "DiffSearch"]), "ddu::DiffSearch");
        // Anything else (sidebar rows don't take focus, so the
        // gpui-component Root context stays on the path).
        assert_eq!(winner(FIND_ACCEL, &["Root"]), "ddu::DiffSearch");
        // Note: gpui's `Not` predicate never evaluates against an
        // empty context stack (its eval guards on a non-empty slice),
        // but the dispatch path always includes at least the Root
        // context, so the unbound case can't occur in the app.
    }

    /// Copy routes by focus, like find: the terminal's own copy is the
    /// chord wherever the terminal is on the dispatch path (its surface
    /// and its find bar alike — the pane's window selection is not the
    /// terminal's business), and the pane's copy is the chord everywhere
    /// else. Being the *winner* is what the menu needs, not merely being
    /// bound: `PopupMenu` hints an item from the action it carries and
    /// accepts only the chord's highest-precedence binding, so while the
    /// pane's binding was predicate-less the terminal's right-click menu
    /// showed a "Copy" item with no ⌘C at all.
    #[test]
    fn copy_routes_by_focus() {
        assert_eq!(
            winner(super::COPY_ACCEL, &["Root", "Terminal"]),
            "ddu::TermCopy"
        );
        assert_eq!(
            winner(super::COPY_ACCEL, &["Root", "Terminal", "TerminalSearch"]),
            "ddu::TermCopy"
        );
        assert_eq!(
            winner(super::COPY_ACCEL, &["Root", "DiffSearch"]),
            "input::Copy"
        );
        assert_eq!(winner(super::COPY_ACCEL, &["Root"]), "input::Copy");
    }

    /// The quick open owns ⌃P even though readline wants it: the whole
    /// point of the choice (⌃P is previous-history in the shell), and
    /// the test that pins the app is allowed to make it.
    #[test]
    fn the_quick_open_owns_its_chord_in_every_context() {
        for stack in [
            vec!["Root"],
            vec!["Root", "Terminal"],
            vec!["Root", "Terminal", "TerminalSearch"],
            vec!["Root", "DiffSearch"],
            vec!["Root", "FileSearch"],
        ] {
            assert_eq!(
                winner(FILE_SEARCH_ACCEL, &stack),
                "ddu::FileSearch",
                "{stack:?}"
            );
        }
    }

    /// What a capture installs, and the whole reason `apply_overrides`
    /// has to append: gpui has no way to take a binding back out, so the
    /// chord a command *left* is disabled by an `Unbind` on it. This is
    /// the layer by hand — `key_bindings` plus the new chord plus the
    /// `Unbind` — and it must both move NewSession and leave every other
    /// chord alone.
    #[test]
    fn a_rebind_replaces_the_chord_it_moved_off() {
        let mut bindings = key_bindings();
        bindings.push(KeyBinding::new(
            "secondary-j",
            crate::app::NewSession,
            None,
        ));
        bindings.push(KeyBinding::new(
            "secondary-n",
            Unbind("ddu::NewSession".into()),
            None,
        ));
        let keymap = Keymap::new(bindings);

        assert_eq!(
            chain_of(&keymap, "secondary-j", &["Root"]),
            vec!["ddu::NewSession".to_string()],
            "the new chord dispatches the command"
        );
        assert!(
            chain_of(&keymap, "secondary-n", &["Root"]).is_empty(),
            "the chord it moved off is free again"
        );
        assert_eq!(
            chain_of(&keymap, "secondary-b", &["Root"]),
            vec!["ddu::ToggleSessions".to_string()],
            "every other chord keeps working"
        );
    }

    /// A second rebind has to take the *first* one out: the layer remembers
    /// the chords it bound and disables them on the next apply, or ⌘J would
    /// keep spawning sessions after the user moved the shortcut to ⌥J.
    #[test]
    fn a_second_rebind_retires_the_first_layer() {
        // Layer 1: NewSession on ⌘J, its builtin ⌘N disabled.
        let mut first = key_bindings();
        first.push(KeyBinding::new(
            "secondary-j",
            crate::app::NewSession,
            None,
        ));
        first.push(KeyBinding::new(
            "secondary-n",
            Unbind("ddu::NewSession".into()),
            None,
        ));
        // Layer 2 (the next apply): the chords layer 1 added are unbound
        // first, then the new spelling is bound.
        let mut second = first;
        second.push(KeyBinding::new(
            "secondary-j",
            Unbind("ddu::NewSession".into()),
            None,
        ));
        second.push(KeyBinding::new(
            "secondary-alt-j",
            crate::app::NewSession,
            None,
        ));
        let keymap = Keymap::new(second);

        assert_eq!(
            chain_of(&keymap, "secondary-alt-j", &["Root"]),
            vec!["ddu::NewSession".to_string()],
            "the current chord runs the command"
        );
        assert!(
            chain_of(&keymap, "secondary-j", &["Root"]).is_empty(),
            "the retired chord is free"
        );
    }

    /// The rule the recorder answers with, without a window: a chord needs
    /// a command modifier, a chord another command holds is refused with
    /// its name, a modifier press only means "still waiting", and escape
    /// means "stop".
    #[test]
    fn capture_refuses_bare_and_taken_chords() {
        let cfg = Config::default();
        let stroke = |s: &str| Keystroke::parse(s).unwrap();

        assert!(matches!(
            captured(&stroke("n"), "new_session", &cfg),
            Captured::Refused(_)
        ));
        assert!(matches!(
            captured(&stroke("shift-n"), "new_session", &cfg),
            Captured::Refused(_)
        ));
        // ⌘R is the changes pane's.
        match captured(&stroke("secondary-r"), "new_session", &cfg) {
            Captured::Refused(reason) => assert!(reason.contains("Toggle changes pane"), "{reason}"),
            _ => panic!("a taken chord must be refused"),
        }
        // A command's *own* chords are not a conflict with itself.
        assert!(matches!(
            captured(&stroke("secondary-n"), "new_session", &cfg),
            Captured::Chord(_)
        ));
        assert!(matches!(
            captured(&stroke("cmd"), "new_session", &cfg),
            Captured::Waiting
        ));
        assert!(matches!(
            captured(&stroke("escape"), "new_session", &cfg),
            Captured::Cancelled
        ));
        match captured(&stroke("ctrl-alt-j"), "new_session", &cfg) {
            Captured::Chord(chord) => assert_eq!(chord, "ctrl-alt-j"),
            _ => panic!("a free chord must be accepted"),
        }
    }

    /// The `keys` map is read through one filter: an id this build does
    /// not know, a chord that does not parse, a chord with no command
    /// modifier, and a chord the user set back to its default are all
    /// ignored rather than bound (or, for the unparseable one, panicked
    /// on by `KeyBinding::new`).
    #[test]
    fn unusable_overrides_are_ignored() {
        let mut cfg = Config::default();
        cfg.keys.insert("no_such_command".into(), "ctrl-alt-j".into());
        cfg.keys.insert("new_session".into(), "nope-".into());
        cfg.keys.insert("add_project".into(), "b".into());
        cfg.keys.insert("toggle_diff".into(), "secondary-r".into());
        cfg.keys
            .insert("toggle_sessions".into(), "ctrl-alt-j".into());
        assert_eq!(
            overrides(&cfg),
            [("toggle_sessions".to_string(), "ctrl-alt-j".to_string())].into(),
            "only the usable, non-default override is bound"
        );

        // Every problem is reported, though — the user asked for these:
        // the unknown id, the modifier-less chord, and the one that is
        // not a keystroke at all. (Setting a command back to its own
        // builtin chord is not a problem: it is dropped, not warned
        // about.)
        let warnings = validate_keys(&cfg);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
    }

    /// A stored override that collides with another command is warned
    /// about at startup (the recorder refuses one at capture time).
    #[test]
    fn a_colliding_override_is_reported() {
        let mut cfg = Config::default();
        cfg.keys.insert("new_session".into(), "ctrl-alt-j".into());
        cfg.keys.insert("add_project".into(), "ctrl-alt-j".into());
        let warnings = validate_keys(&cfg);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().all(|w| w.contains("also")), "{warnings:?}");
    }

    /// Every builtin chord has to parse: a typo in the table is a panic
    /// inside `KeyBinding::new` at the first install, and a command whose
    /// chord the user may not rebind *to* is a trap for the recorder.
    #[test]
    fn every_builtin_chord_parses_and_is_distinct() {
        let mut seen: Vec<(String, Option<&str>)> = Vec::new();
        for command in COMMANDS {
            assert!(
                !command.chords.is_empty(),
                "{} has no chord to rebind",
                command.id
            );
            for chord in command.chords {
                let stroke = Keystroke::parse(chord.chord)
                    .unwrap_or_else(|_| panic!("{}: {}", command.id, chord.chord));
                let key = (stroke.unparse(), chord.context);
                assert!(
                    !seen.contains(&key),
                    "{} binds {:?} twice",
                    command.id,
                    key
                );
                seen.push(key);
            }
        }
        for reserved in RESERVED {
            Keystroke::parse(reserved.chord.chord)
                .unwrap_or_else(|_| panic!("{}: {}", reserved.label, reserved.chord.chord));
        }
        // No command chord is a reserved one (a capture would refuse it).
        for command in COMMANDS {
            for chord in command.chords {
                let stroke = Keystroke::parse(chord.chord).unwrap();
                assert_eq!(
                    chord_taken_by(&stroke, &Config::default(), command.id),
                    None,
                    "{}: {}",
                    command.id,
                    chord.chord
                );
            }
        }
    }

    /// Every action the app handles has a chord. Written out rather than
    /// derived, because that is the point: the day a command is dropped
    /// from the table (which nearly happened to `CloseSession` when the
    /// table grew the settings page's vocabulary), ⌘W silently stops
    /// working and nothing else notices. `input::Copy` is gpui-base's —
    /// the pane's own copy — and the two bars' stepping lives in
    /// [`RESERVED`].
    #[test]
    fn every_handled_action_is_bound() {
        let mut bound: Vec<String> = key_bindings()
            .iter()
            .map(|binding| binding.action().name().to_string())
            .collect();
        bound.sort();
        bound.dedup();
        let mut expected: Vec<String> = [
            "ddu::AddProject",
            "ddu::CloseSession",
            "ddu::CloseSettings",
            "ddu::CopyFileContents",
            "ddu::CopyFilePath",
            "ddu::DiffSearch",
            "ddu::DiffSearchNext",
            "ddu::DiffSearchPrev",
            "ddu::FileSearch",
            "ddu::FileSearchNext",
            "ddu::FileSearchPrev",
            "ddu::FontLarger",
            "ddu::FontSmaller",
            "ddu::NewSession",
            "ddu::OpenSettings",
            "ddu::Quit",
            "ddu::SelectSession1",
            "ddu::SelectSession2",
            "ddu::SelectSession3",
            "ddu::SelectSession4",
            "ddu::SelectSession5",
            "ddu::SelectSession6",
            "ddu::SelectSession7",
            "ddu::SelectSession8",
            "ddu::SelectSession9",
            "ddu::TermBacktab",
            "ddu::TermCopy",
            "ddu::TermPaste",
            "ddu::TermSearch",
            "ddu::TermSearchNext",
            "ddu::TermSearchPrev",
            "ddu::TermTab",
            "ddu::ToggleDiff",
            "ddu::ToggleDiffTree",
            "ddu::ToggleSessions",
            "ddu::ToggleViewMode",
            "input::Copy",
        ]
        .iter()
        .map(|name| name.to_string())
        .collect();
        expected.sort();
        assert_eq!(bound, expected);
    }

    /// What the settings row prints for a rebindable command, and what
    /// the tooltips print: one spelling, the platform's.
    #[test]
    fn labels_are_the_platforms_spelling() {
        let cfg = Config::default();
        let command = COMMANDS.iter().find(|c| c.id == "new_session").unwrap();
        assert_eq!(super::shortcut(command, &cfg).text, chord_label("secondary-n"));
        assert!(!super::shortcut(command, &cfg).custom);
        assert!(
            chord_label("secondary-n").ends_with('N'),
            "{}",
            chord_label("secondary-n")
        );
    }
}
