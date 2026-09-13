//! The shell's frame: the three-pane layout with its two splitters, the
//! window-level action handlers, the layers hanging off the root, and the
//! splitter-width healing a container resize needs.

use super::*;

impl AppView {

    /// Re-assert the recorded sidebar / diff widths after the *window*
    /// changed size: a resize that lands while every slot was still
    /// pinned (startup's burst, a monitor switch) leaves the panels at
    /// proportions nobody asked for.
    ///
    /// This must **not** run from `render`. A drag changes a panel's size
    /// several times a second while the persisted record only catches up a
    /// tick later, so a render in between (a stream frame, the 1 s clock,
    /// the poll) would "correct" a live drag back to the width it started
    /// at — the divider that sometimes refuses to be dragged. Window
    /// bounds change for reasons a drag cannot, which is exactly the
    /// trigger this healing wants.
    fn heal_splitter_widths(&mut self, cx: &mut Context<Self>) {
        // A drag redistributes within one container: it never changes
        // this, which is what keeps the healing from fighting one.
        let shell_container = self.shell_state.read(cx).container_size();
        let panes_container = self.panes_state.read(cx).container_size();
        // Re-measured rather than nudged. gpui-base pins a slot at its
        // *first* measured bounds, and a slot measured while the group had
        // another shape — the center alone, before the diff pane existed,
        // where it measures the whole container — keeps that number for
        // good. A drag reads the pair as its starting widths and hands the
        // changed space to the sibling, so a pair that is stale (or that no
        // longer sums to the container) puts the divider wherever that
        // arithmetic lands, not at the pointer. Dropping the state re-pins
        // both slots against a layout with the current shape, and the sized
        // panel re-asserts its recorded width as its initial size.
        if shell_container > px(0.)
            && (shell_container != self.healed_shell_at
                || self.shell_state.read(cx).sizes().len() != self.shell_len())
        {
            self.healed_shell_at = shell_container;
            self.shell_state.update(cx, |state, _| state.clear());
        }
        if panes_container > px(0.)
            && (panes_container != self.healed_panes_at
                || self.panes_state.read(cx).sizes().len() != self.panes_len())
        {
            self.healed_panes_at = panes_container;
            self.panes_state.update(cx, |state, _| state.clear());
        }
    }

    /// How many slots a splitter has this frame — what its state is
    /// re-measured for when the two disagree.
    fn shell_len(&self) -> usize {
        if self.show_sessions { 2 } else { 1 }
    }

    fn panes_len(&self) -> usize {
        if self.show_diff { 2 } else { 1 }
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The root tracks the window fallback focus: every pane focuses
        // this handle on click (so the terminal cursor goes hollow), and
        // with it in the element tree the global shortcuts (⌘N/⌘T/⌘B/…)
        // keep a dispatch path no matter what was clicked last.
        v_flex()
            .id("app-root")
            .relative()
            .track_focus(&self.window_focus)
            .size_full()
            .bg(cx.theme().background)
            // Left-drag gesture fence: keep the window repainting while a
            // selection drag is in flight. `TextSelectionLayer` observes
            // mouse events via `Window::on_mouse_event`, whose listeners
            // live only for one frame after a render — without a steady
            // frame stream the remaining move events of a drag are dropped
            // and selection freezes until some unrelated repaint. Terminal
            // selection uses persistent element callbacks, so it never had
            // this problem; this fence gives the window-level listeners the
            // same continuous frame stream.
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, _| {
                    this.selection_active = true;
                }),
            )
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, window, _| {
                if this.selection_active {
                    window.refresh();
                }
            }))
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.selection_active = false;
                }),
            )
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.selection_active = false;
                }),
            )
            .on_action(cx.listener(|_, _: &OpenSettings, _, cx| ui::settings::open(cx)))
            .on_action(|_: &FontLarger, _, cx| ui::settings::bump_font_size(1., cx))
            .on_action(|_: &FontSmaller, _, cx| ui::settings::bump_font_size(-1., cx))
            .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                if window.has_active_dialog(cx) {
                    return;
                }
                // ⌘N on an empty workspace is a no-op: spawning needs
                // an active project to create the session in.
                if this.projects.is_empty() {
                    return;
                }
                this.spawn_session(window, cx);
            }))
            .on_action(cx.listener(|this, _: &AddProject, window, cx| {
                if window.has_active_dialog(cx) {
                    return;
                }
                // Same folder-picker flow as the sidebar button.
                this.add_project(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession1, window, cx| {
                this.select_session(this.current_project, 0, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession2, window, cx| {
                this.select_session(this.current_project, 1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession3, window, cx| {
                this.select_session(this.current_project, 2, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession4, window, cx| {
                this.select_session(this.current_project, 3, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession5, window, cx| {
                this.select_session(this.current_project, 4, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession6, window, cx| {
                this.select_session(this.current_project, 5, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession7, window, cx| {
                this.select_session(this.current_project, 6, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession8, window, cx| {
                this.select_session(this.current_project, 7, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession9, window, cx| {
                this.select_session(this.current_project, 8, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseSession, window, cx| {
                if window.has_active_dialog(cx) {
                    window.close_dialog(cx);
                    return;
                }
                let p = this.current_project;
                let six = this.current_session;
                this.request_close_session(p, six, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSessions, _, cx| this.toggle_sessions(cx)))
            .on_action(cx.listener(|this, _: &ToggleDiff, window, cx| this.toggle_diff(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleDiffTree, _, cx| this.toggle_diff_tree(cx)))
            .on_action(cx.listener(|this, _: &ToggleViewMode, _, cx| this.toggle_view_mode(cx)))
            // The diff pane's find bar. ⌘G/⌘⇧G resolve only while the
            // bar's input holds focus (the "DiffSearch" key context);
            // the handlers live here, not on the bar, so they keep
            // working if focus drifts to the pane mid-search.
            .on_action(cx.listener(|this, _: &DiffSearch, window, cx| {
                this.open_diff_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &DiffSearchNext, _, cx| {
                this.diff_search_step(false, cx);
            }))
            .on_action(cx.listener(|this, _: &DiffSearchPrev, _, cx| {
                this.diff_search_step(true, cx);
            }))
            // The file tree's quick open. ⌘G/⌘⇧G resolve only while its
            // input holds focus ("FileSearch"); the handlers live here so
            // they keep working if focus drifts mid-search.
            .on_action(cx.listener(|this, _: &FileSearch, window, cx| {
                this.open_file_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FileSearchNext, _, cx| {
                this.file_search_step(false, cx);
            }))
            .on_action(cx.listener(|this, _: &FileSearchPrev, _, cx| {
                this.file_search_step(true, cx);
            }))
            // ⌘Q: confirm dialog, then graceful shutdown — live agents
            // get Ctrl-C, their resume ids land in state.json, then the
            // process exits.
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.request_quit(window, cx);
            }))
            // ⌘C outside the terminal: copy the diff pane's window
            // text selection (the terminal has its own TermCopy path
            // via its deeper key context).
            .on_action(|_: &input::Copy, window, cx| {
                let text = TextSelection::selected_text(window, cx);
                if text.is_empty() {
                    cx.propagate();
                    return;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            })
            // The pane's and the tree's context-menu copies, as chords
            // (see `keys.rs`): both read the *current* selection, so the
            // menu item, the key message and the clipboard agree even if
            // the selection moved between opening the menu and clicking.
            .on_action(cx.listener(|this, _: &CopyFilePath, _, cx| {
                if let Some(path) = this.current_diff_path() {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.to_owned()));
                }
            }))
            .on_action(cx.listener(|this, _: &CopyFileContents, _, cx| {
                let text = this.selected_file_text();
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }))
            // Window-scoped text selection (the diff pane's
            // `SelectableText` runs register here). Must prepaint
            // before any of them — first child of the root.
            .child(TextSelectionLayer)
            .child(ui::title_bar::bar(self))
            .child({
                // Two nested splitters. The sidebar column owns a status
                // strip, so the LEFT divider runs to the window's bottom
                // edge; the terminal/changes region wraps its splitter
                let panes_min = center_min() + if self.show_diff { diff_min() } else { 0. } + ui::scaled(8.);
                // The diff pane flexes with the window: its drag cap
                // scales with the viewport (a fixed cap reads cramped
                // on a big display), floored at DIFF_MAX.
                let diff_max = (window.viewport_size().width.as_f32() * 0.6).max(diff_max());
                // A container (window) resize proportionally rescales
                // every splitter panel that has a recorded size —
                // gpui-base's `adjust_to_container_size` bails only
                // while some panel is unpinned, and its
                // `update_panel_size` pins EVERY slot at its first
                // measured bounds. Keep the flex slots (the region,
                // the center pane) unpinned so the recorded sidebar /
                // diff widths survive window resizes verbatim; the
                // flex slot absorbs the whole delta.
                self.shell_state.update(cx, |state, cx| {
                    if state.sizes().len() > 1 {
                        state.reset_panel(1, cx);
                    }
                });
                self.panes_state.update(cx, |state, cx| {
                    if state.sizes().len() > 1 {
                        state.reset_panel(0, cx);
                    }
                });
                // Heal drift baked in while every slot was still pinned
                // (the launch resize burst): re-assert the recorded
                // widths, once per container size. A drag cannot trigger
                // it — a drag leaves the container alone — which is what
                // keeps a mid-drag frame from yanking the divider back.
                self.heal_splitter_widths(cx);
                let mut shell = h_resizable("shell").with_state(&self.shell_state);
                if self.show_sessions {
                    shell = shell.child(
                        resizable_panel()
                            .size(self.last_sidebar_w())
                            .flex_none()
                            .size_range(px(sidebar_min())..px(sidebar_max()))
                            .child(
                                v_flex()
                                    .size_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .child(
                                        self.sidebar
                                            .clone()
                                            .cached(ui::session_panel::root_style()),
                                    ),
                                    )
                                    .child(ui::status_bar::render_sidebar(self, cx)),
                            ),
                    );
                }
                let mut panes = h_resizable("panes").with_state(&self.panes_state);
                panes = panes.child(
                    resizable_panel()
                        .size_range(px(center_min())..px(f32::MAX))
                        .child(
                            div()
                                .debug_selector(|| "pane-center".into())
                                .h_full()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .child(self.terminal_pane.clone().cached(ui::terminal_panel::root_style())),
                        ),
                );
                if self.show_diff {
                    // `.flex_none()`, like the sidebar: this is the sized
                    // pane, and an unsized flex sibling (the center) gets a
                    // flex *base* of the whole container — so leaving this
                    // one growable made the two shrink against each other
                    // and the divider land wherever that math put it
                    // instead of at the pointer (and the recorded width
                    // never matched the laid-out one). Held at its recorded
                    // width, the center absorbs the window delta.
                    //
                    // That width is the recorded one capped to what the
                    // container can hold beside the terminal's minimum: the
                    // record keeps the preference, so a wider window restores
                    // it. This is the panel's *initial* size, which is what
                    // the layout reads right after the state in
                    // `heal_splitter_widths` is dropped (a container resize,
                    // a pane toggle) — the steady state reads the record.
                    let room =
                        self.panes_state.read(cx).container_size() - px(center_min() + ui::scaled(8.));
                    let diff_w = if room > px(0.) {
                        self.last_diff_w().min(room)
                    } else {
                        self.last_diff_w()
                    };
                    panes = panes.child(
                        resizable_panel()
                            .size(diff_w)
                            .flex_none()
                            .size_range(px(diff_min())..px(diff_max))
                            .child(
                                div()
                                    .debug_selector(|| "pane-diff".into())
                                    .size_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    // *Uncached*, unlike the sidebar, the
                                    // terminal pane and the breadcrumb. This
                                    // pane is the only place in the app that
                                    // puts text in gpui's window selection
                                    // (its rows and the rendered document are
                                    // `SelectableText`/`TextView`), and that
                                    // layer keeps a participant only while it
                                    // re-registers: `TextSelectionLayer` sweeps
                                    // whatever did not register *this* frame
                                    // (`finish_frame`), and a cached subtree
                                    // registers nothing on the frames it
                                    // replays. A cached pane therefore lost a
                                    // live selection — the highlight blinked
                                    // off the moment a stream frame arrived —
                                    // and, for the rendered document, the sweep
                                    // cleared a participant whose clear
                                    // notifies the text view, so every stream
                                    // frame re-rendered the pane and painted
                                    // the whole window twice (measured: 5.5% →
                                    // 3% CPU, 20 → 10.5 root renders per 2 s
                                    // window, session for session). Painting
                                    // the pane every frame is what the library
                                    // assumes of a selectable surface; it costs
                                    // the pane's own rows, and nothing else.
                                    .child(self.diff_pane.clone()),
                            ),
                    );
                }
                let region = v_flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(div().flex_1().min_h_0().overflow_hidden().child(panes))
                    .child(ui::status_bar::render_center(self, cx));
                div().flex_1().min_h_0().overflow_hidden().child(
                    shell.child(
                        resizable_panel()
                            .size_range(px(panes_min)..px(f32::MAX))
                            .child(region),
                    ),
                )
            })
            // The quick open's palette (⌘P): a scrim and a card over the
            // whole workspace, mounted *above* the panels and below the
            // dialog layer, so a confirm dialog opened from a palette (or
            // while one is up) still paints over everything.
            .child(ui::palette::render(self, cx))
            // Overlay layers (anchored, no layout impact): dialogs opened via
            // window.open_dialog / open_alert_dialog are hosted here —
            // gpui-kit requires the app to render these layers.
            .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
            // Exit in progress (confirmed close/⌘Q): a dimming overlay
            // while agents are interrupted and their resume ids saved.
            .when(self.shutting_down, |el| {
                let n = self.running_terms().len();
                let detail = if n > 0 {
                    format!(
                        "Stopping {n} session{} and saving resume ids…",
                        if n == 1 { "" } else { "s" }
                    )
                } else {
                    "Saving session ids…".to_string()
                };
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui_kit::black().opacity(0.45))
                        .child(
                            // The card, not bare text: the message must
                            // read at a glance while the window dies.
                            v_flex()
                                .items_center()
                                .gap_2()
                                .px_8()
                                .py_6()
                                .rounded_lg()
                                .bg(cx.theme().popover)
                                .border_1()
                                .border_color(cx.theme().border)
                                .shadow_lg()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .text_base()
                                        .text_color(cx.theme().popover_foreground)
                                        .child(Spinner::new())
                                        .child("Exiting…"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(detail),
                                ),
                        ),
                )
            })
    }
}
