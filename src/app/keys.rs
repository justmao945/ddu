//! The app's key bindings in one place: the platform chords the terminal
//! surface shares with the shell it runs, the accelerator labels
//! user-facing text prints, and the binding table `AppView::new`
//! installs — which the routing tests below pin, precedence included.

use super::*;

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
/// Shortcut label for user-facing text (`⌘N` on macOS, `Ctrl+N` elsewhere).
pub(crate) fn accel_hint(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
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
pub(super) fn key_bindings() -> Vec<KeyBinding> {
    vec![
        // Secondary (⌘ / ⌃) + N spawns the default launcher in the
        // active project (guarded: with no projects there is nothing
        // to spawn into); + O adds a project via the folder picker;
        // + T toggles the diff file tree under the project tree.
        KeyBinding::new("secondary-n", NewSession, None),
        KeyBinding::new("secondary-o", AddProject, None),
        KeyBinding::new("secondary-t", ToggleDiffTree, None),
        // Quick open: the sidebar's file search.
        KeyBinding::new(FILE_SEARCH_ACCEL, FileSearch, None),
        KeyBinding::new("secondary-b", ToggleSessions, None),
        KeyBinding::new("secondary-r", ToggleDiff, None),
        KeyBinding::new("secondary-w", CloseSession, None),
        // Cycle the right pane's surface: diff hunks → whole file →
        // rendered Markdown (Markdown files only).
        KeyBinding::new("secondary-shift-m", ToggleViewMode, None),
        // Secondary +/− (with their shifted variants) zoom the
        // terminal font size; persisted like the settings field.
        KeyBinding::new("secondary-=", FontLarger, None),
        KeyBinding::new("secondary-+", FontLarger, None),
        KeyBinding::new("secondary--", FontSmaller, None),
        KeyBinding::new("secondary-_", FontSmaller, None),
        // Secondary 1..9: select the Nth session in the current
        // project. Prefixed so bare digits keep reaching the PTY.
        KeyBinding::new("secondary-1", SelectSession1, None),
        KeyBinding::new("secondary-2", SelectSession2, None),
        KeyBinding::new("secondary-3", SelectSession3, None),
        KeyBinding::new("secondary-4", SelectSession4, None),
        KeyBinding::new("secondary-5", SelectSession5, None),
        KeyBinding::new("secondary-6", SelectSession6, None),
        KeyBinding::new("secondary-7", SelectSession7, None),
        KeyBinding::new("secondary-8", SelectSession8, None),
        KeyBinding::new("secondary-9", SelectSession9, None),
        // Terminal-scoped: these beat gpui-component Root's global
        // Tab/Shift-Tab focus cycling (deeper key context wins), so
        // the PTY gets real tab/backtab bytes and focus never jumps
        // to sidebar buttons mid-session. Paste is unbound globally.
        KeyBinding::new("tab", TermTab, Some("Terminal")),
        KeyBinding::new("shift-tab", TermBacktab, Some("Terminal")),
        KeyBinding::new(PASTE_ACCEL, TermPaste, Some("Terminal")),
        // Copy grabs the mouse selection when one exists (the
        // handler propagates otherwise); PTYs never see it.
        KeyBinding::new(COPY_ACCEL, TermCopy, Some("Terminal")),
        // Outside the terminal, copy grabs the active diff-pane text
        // selection (window-scoped `TextSelection`); the handler
        // propagates when nothing is selected.
        //
        // `!Terminal`, never a bare `None`: a predicate-less binding
        // always scores the full context stack, so it *ties* the named
        // context at the focused element and then wins the
        // later-binding tiebreak. With a `None` here the terminal's
        // ⌘C dispatched the pane's copy first — and the terminal's
        // right-click menu, which hints an item from the action it
        // carries and accepts only the chord's *winner*
        // (`PopupMenuItem::action`), printed no chord for `TermCopy` at
        // all. `!Terminal` (true only while the terminal is nowhere on
        // the dispatch path) makes the two bindings mutually exclusive,
        // so the winner needs no tiebreak at all — the same fix, for the
        // same reason, as `FIND_ACCEL` below.
        KeyBinding::new(COPY_ACCEL, input::Copy, Some("!Terminal")),
        // The two file-copy commands the tree's and the pane's context
        // menus carry: both act on the pane's selected file, so the
        // menus can show the chord they run. App-level (no key
        // context): the chords are the same wherever the pointer is,
        // and neither is one a shell reads.
        KeyBinding::new(COPY_PATH_ACCEL, CopyFilePath, None),
        KeyBinding::new(COPY_CONTENTS_ACCEL, CopyFileContents, None),
        // Two find bars share one chord by focus. gpui ranks a
        // binding by the deepest stack slice its predicate needs: a
        // named context sitting at the focused element scores len, a
        // predicate-less binding always scores len, and ties go to
        // the later binding — so a bare None here would permanently
        // out-rank the Terminal binding below. `!Terminal` matches
        // one slice *shallower* than `Terminal` itself (the largest
        // slice excluding it), so the positive binding always wins
        // by exactly one level whenever the terminal surface or its
        // find bar holds focus, and the diff binding wins anywhere
        // else ("Terminal" absent from every slice). When a bar's
        // input already holds focus its own action refocuses it
        // with the query selected, matching platform find bars.
        KeyBinding::new(FIND_ACCEL, TermSearch, Some("Terminal")),
        KeyBinding::new(FIND_ACCEL, DiffSearch, Some("!Terminal")),
        // The diff pane's find bar: once its input holds focus the
        // "DiffSearch" context is on the dispatch path. Enter/
        // Shift-Enter come from the input itself (it dispatches
        // the `Enter` action), handled on the bar in
        // `ui::diff_panel`.
        KeyBinding::new("secondary-g", DiffSearchNext, Some("DiffSearch")),
        KeyBinding::new("secondary-shift-g", DiffSearchPrev, Some("DiffSearch")),
        // The quick open's: while its input holds focus the "FileSearch"
        // context is on the dispatch path, and the plain arrows belong
        // to the input's own caret. Enter/Escape come from the input.
        KeyBinding::new("secondary-g", FileSearchNext, Some("FileSearch")),
        KeyBinding::new("secondary-shift-g", FileSearchPrev, Some("FileSearch")),
        // The palette's arrows. The field is an input, and gpui-base
        // claims up/down for every input in the deeper "Input" context —
        // but it *registers* the move actions only for a multi-line
        // field, so a single-line one falls through to the palette,
        // which is one context shallower. This is the whole reason the
        // palette can navigate with the arrows every other one uses.
        KeyBinding::new("down", FileSearchNext, Some("FileSearch")),
        KeyBinding::new("up", FileSearchPrev, Some("FileSearch")),
        // The terminal bar's match-cycling: its "TerminalSearch"
        // context (set on the bar in `ui::terminal_panel`) is deeper
        // than the surface's "Terminal", so these win while its
        // input holds focus.
        KeyBinding::new("secondary-g", TermSearchNext, Some("TerminalSearch")),
        KeyBinding::new("secondary-shift-g", TermSearchPrev, Some("TerminalSearch")),
        // The standalone settings window: Escape / secondary+W close
        // it (the deeper context beats the global close-session one).
        KeyBinding::new("escape", CloseSettings, Some("SettingsWindow")),
        KeyBinding::new("secondary-w", CloseSettings, Some("SettingsWindow")),
    ]
}
#[cfg(test)]
mod tests {
    use super::{
        COPY_ACCEL, COPY_CONTENTS_ACCEL, COPY_PATH_ACCEL, FILE_SEARCH_ACCEL, FIND_ACCEL,
        PASTE_ACCEL, key_bindings,
    };
    use gpui_kit::{KeyContext, Keymap, Keystroke};

    /// The fallback chain gpui would dispatch for `keystroke`, in
    /// precedence order, given a context stack (bottom → top, as the
    /// dispatch tree builds it).
    fn chain(keystroke: &str, stack: &[&str]) -> Vec<String> {
        let keymap = Keymap::new(key_bindings());
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
            chain(COPY_ACCEL, &["Root", "Terminal"]),
            vec!["ddu::TermCopy"]
        );
        assert_eq!(
            chain(COPY_ACCEL, &["Root", "Terminal", "TerminalSearch"]),
            vec!["ddu::TermCopy"]
        );
        assert_eq!(chain(COPY_ACCEL, &["Root"]), vec!["input::Copy"]);
    }

    /// The tree's and the pane's copy commands are *bound*, not merely
    /// labelled: a context menu renders an item's key hint from the
    /// action the item carries, so an unbound action shows a menu entry
    /// with no chord — which is what a right-click used to look like.
    /// Both chords work inside the terminal too: neither is one a shell
    /// reads (⌥C alone is readline's capitalize-word, ⌃⌥C is free).
    #[test]
    fn the_file_copy_commands_are_bound_chords() {
        for stack in [vec!["Root"], vec!["Root", "Terminal"]] {
            assert_eq!(
                winner(COPY_PATH_ACCEL, &stack),
                "ddu::CopyFilePath",
                "{COPY_PATH_ACCEL} with {stack:?}"
            );
            assert_eq!(
                winner(COPY_CONTENTS_ACCEL, &stack),
                "ddu::CopyFileContents",
                "{COPY_CONTENTS_ACCEL} with {stack:?}"
            );
        }
    }

    /// The palette's arrows are the app's — in the palette's own context,
    /// and nowhere else, so the terminal keeps its shell's history keys.
    ///
    /// Its field is a single-line input: gpui-base binds up/down for any
    /// input in the deeper "Input" context, but *registers handlers* for
    /// them only when the input is multi-line, so even if the library's
    /// binding is in the keymap the palette's action is the one that
    /// lands and the cursor is what moves. What a test can pin is the
    /// app's half of that — the library's keymap is not visible from
    /// here.
    #[test]
    fn the_palette_steps_with_the_arrows_the_field_gives_up() {
        assert_eq!(winner("up", &["Root", "FileSearch"]), "ddu::FileSearchPrev");
        assert_eq!(
            winner("down", &["Root", "FileSearch"]),
            "ddu::FileSearchNext"
        );
        assert_eq!(winner("up", &["Root", "Terminal"]), "");
        assert_eq!(winner("down", &["Root", "Terminal"]), "");
        assert_eq!(winner("up", &["Root"]), "");
    }

    /// The quick open is the app's chord everywhere — including in the
    /// terminal, where the shell would otherwise read ⌃P as "previous
    /// history". It is a deliberate trade, like ⌃R before it: the test
    /// states it so a later binding cannot take the chord back by
    /// accident.
    #[test]
    fn the_quick_open_owns_its_chord_in_every_context() {
        for stack in [
            vec!["Root"],
            vec!["Root", "Terminal"],
            vec!["Root", "Terminal", "TerminalSearch"],
            vec!["Root", "Input"],
        ] {
            assert_eq!(
                winner(FILE_SEARCH_ACCEL, &stack),
                "ddu::FileSearch",
                "{FILE_SEARCH_ACCEL} with {stack:?}"
            );
        }
    }

    /// The terminal shares the keyboard with the shell it runs, so the
    /// chords readline and the OS own must stay unbound inside it:
    /// Ctrl-C is SIGINT, Ctrl-D ends input, Ctrl-Z suspends, and
    /// Ctrl-V/ Ctrl-F/ Ctrl-\\ belong to readline. macOS is exempt —
    /// the PTY never sees ⌘ chords, so the shared chords are already
    /// out of the PTY's way.
    #[test]
    fn shell_control_keys_stay_with_the_shell() {
        if cfg!(target_os = "macos") {
            return;
        }
        for key in ["ctrl-c", "ctrl-d", "ctrl-v", "ctrl-f", "ctrl-z", "ctrl-\\"] {
            assert_eq!(
                winner(key, &["Root", "Terminal"]),
                "",
                "{key} must reach the shell, not an app action"
            );
        }
        // What the terminal surface does claim is copy/paste/find, in
        // the shifted space no shell reads.
        let term = ["Root", "Terminal"];
        assert!(chain(COPY_ACCEL, &term).contains(&"ddu::TermCopy".to_string()));
        assert!(chain(PASTE_ACCEL, &term).contains(&"ddu::TermPaste".to_string()));
        assert_eq!(winner(FIND_ACCEL, &term), "ddu::TermSearch");
    }
}
