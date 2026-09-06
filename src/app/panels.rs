//! Panel toggles: sidebar, changes pane, diff-tree layer — plus the
//! remembered last-dragged widths each toggle restores.

use super::*;

impl AppView {
    /// Toggle the sidebar, keeping the splitter slot list in sync.
    pub(crate) fn toggle_sessions(&mut self, cx: &mut Context<Self>) {
        self.set_sessions(!self.show_sessions, cx);
    }

    pub(crate) fn set_sessions(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.show_sessions {
            return;
        }
        self.show_sessions = on;
        self.persist(cx);
        let restore_w = self.last_sidebar_w();
        if !on {
            // Capture before removal: after `remove_panel(0)` slot 0 is
            // the center+diff region, not the sidebar.
            self.last_sidebar_size = self.shell_state.read(cx).sizes().first().copied();
        }
        // The panes (terminal/diff) live in their own splitter, so a
        // sidebar insert/remove never rescales them — no re-pin needed.
        self.shell_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(restore_w), Some(0), cx);
            } else {
                state.remove_panel(0, cx);
            }
        });
        cx.notify();
    }

    /// Toggle the diff panel (slot sits after the always-present center).
    pub(crate) fn toggle_diff(&mut self, cx: &mut Context<Self>) {
        self.set_diff(!self.show_diff, cx);
    }

    /// Toggle the diff file tree layer under the project tree.
    pub(crate) fn toggle_diff_tree(&mut self, cx: &mut Context<Self>) {
        self.show_diff_tree = !self.show_diff_tree;
        self.persist(cx);
        cx.notify();
    }

    pub(crate) fn set_diff(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.show_diff {
            return;
        }
        // Inner splitter slots: the terminal is 0, the diff 1.
        const DIFF_IX: usize = 1;
        self.show_diff = on;
        self.persist(cx);
        let restore_w = self.last_diff_w();
        if !on {
            // Capture before removal: the slot shifts after `remove_panel`.
            self.last_diff_size = self.panes_state.read(cx).sizes().get(DIFF_IX).copied();
        }
        // The sidebar lives in the outer splitter, so a diff
        // insert/remove never rescales it — no re-pin needed.
        self.panes_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(restore_w), Some(DIFF_IX), cx);
            } else {
                state.remove_panel(DIFF_IX, cx);
            }
        });
        cx.notify();
    }

    pub(super) fn last_sidebar_w(&self) -> Pixels {
        self.last_sidebar_size
            .filter(|w| *w >= px(SIDEBAR_MIN))
            .unwrap_or(px(SIDEBAR_DEFAULT))
    }

    pub(super) fn last_diff_w(&self) -> Pixels {
        self.last_diff_size
            .filter(|w| *w >= px(DIFF_MIN))
            .unwrap_or(px(DIFF_DEFAULT))
    }
}
