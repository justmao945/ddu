//! Diff data flow: the periodic working-tree poll, stale-result
//! guarding and selection re-pinning.

use std::time::Duration;

use super::*;

impl AppView {
    pub(super) fn reset_diff(&mut self) {
        self.diff = None;
        self.diff_error = None;
        self.diff_file = 0;
        self.diff_tree_closed.clear();
        self.diff_tree_scroll.set_offset(point(px(0.), px(0.)));
        self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
    }

    pub(super) fn apply_diff(&mut self, result: anyhow::Result<GitDiff>) {
        let selected = self
            .diff
            .as_ref()
            .and_then(|d| d.files.get(self.diff_file))
            .map(|f| f.path.clone());
        // First paint after a cold start: the persisted selection wins
        // when the path still exists in the working tree; afterwards
        // switching projects keeps overwriting it via `diff_file`.
        let seed = if self.diff_seed_path.is_some() && self.diff.is_none() {
            self.diff_seed_path.clone()
        } else {
            None
        };
        let selected = selected.or(seed);
        match result {
            Ok(diff) => {
                let next = selected
                    .as_ref()
                    .and_then(|path| diff.files.iter().position(|f| &f.path == path))
                    .unwrap_or(0);
                if selected.as_ref() != diff.files.get(next).map(|f| &f.path) {
                    self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
                }
                self.diff_file = next;
                self.diff = Some(diff);
                self.diff_error = None;
            }
            Err(err) => {
                self.diff = None;
                self.diff_error = Some(err.to_string());
            }
        }
        self.diff_seed_path = None;
    }

    /// Kick off one diff reload; results newer than any in-flight one win.
    pub(crate) fn reload_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_seq += 1;
        let seq = self.diff_seq;
        let path = self.current_project().path.clone();
        let this = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_spawn(async move { git::head_diff(&path) })
                .await;
            if let Err(e) = &result {
                eprintln!("[ddu] diff err: {e:#}");
            }
            let _ = this.update(cx, |v, cx| {
                if v.diff_seq == seq {
                    v.apply_diff(result);
                    cx.notify();
                }
            });
            anyhow::Ok(())
        })
        .detach();
    }

    /// Periodic working-tree poll so the diff panel stays fresh (M3
    /// acceptance: edits show up within 3s).
    pub(super) fn start_diff_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(DIFF_POLL_SECS))
                    .await;
                let Some(view) = this.upgrade() else { break };
                let (path, seq) =
                    view.read_with(cx, |v, _| (v.current_project().path.clone(), v.diff_seq));
                let result = cx
                    .background_spawn(async move { git::head_diff(&path) })
                    .await;
                this.update(cx, |v, cx| {
                    if v.diff_seq == seq {
                        v.apply_diff(result);
                        cx.notify();
                    }
                })?;
            }
            anyhow::Ok(())
        })
        .detach();
    }
}
