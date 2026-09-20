//! The overview strip in the pane's scrollbar column: one mark per run
//! of changed rows, positioned in relative lengths so the strip stays
//! put through a resize.

use super::*;


/// The change overview: a strip in the scrollbar's own column showing where
/// the stream's changed runs sit in the whole stream, one mark per run. It
/// exists to be *used*: clicking a mark scrolls that run to the top, which is
/// how a 200k-line file is navigated. Nothing is drawn for a stream with no
/// changed rows, and the viewport is not drawn at all — the pane's scrollbar
/// is in this very column and says it better.
///
/// It is an **overlay**, not a column: mounted on the rows' host as an
/// `absolute` child (before `.scrollbar`, so the scrollbar paints over it),
/// it costs the rows no width at all. Its inset is the rows' own `p_2`, so
/// its height is exactly the content viewport's — which is what makes a
/// mark's share of the stream land where the row actually is.
///
/// Two columns, not one: additions take the left half, removals the right
/// (`ADDED_COLUMN`). A replacement — a deleted line with its added
/// counterpart a row below — puts the two marks side by side instead of
/// stacking them, which is what a single column did: the second mark covered
/// the first, and every replacement read as one colour.
///
/// Positions are relative lengths (share of the content), not pixels:
/// the element's height is whatever the pane's layout gives it, and
/// dividing by a height measured a frame earlier would drift on resize.
pub(super) fn scroll_overview(
    stream: &RowStream<'_>,
    sizes: &[gpui_kit::Size<Pixels>],
    cx: &mut Context<AppView>,
) -> AnyElement {
    let marks = stream.marks();
    // A stream with nothing changed, or no rows at all, has no overview.
    let total: f32 = sizes.iter().map(|s| f32::from(s.height)).sum();
    if marks.is_empty() || total <= 0. {
        return div().into_any_element();
    }
    let colors = (
        cx.theme().green.opacity(0.75),
        cx.theme().red.opacity(0.75),
    );
    // Marks are in row order, so one pass down the declared heights
    // places them all.
    let mut bars: Vec<AnyElement> = Vec::with_capacity(marks.len());
    let (mut cursor, mut top) = (0usize, 0f32);
    for mark in marks {
        while cursor < mark.row as usize {
            top += f32::from(sizes.get(cursor).map_or(px(0.), |s| s.height));
            cursor += 1;
        }
        let mut height = 0f32;
        for row in cursor..(cursor + mark.rows as usize) {
            height += f32::from(sizes.get(row).map_or(px(0.), |s| s.height));
        }
        let (color, side) = match mark.kind {
            ddu_diff::ChangeKind::Added => (colors.0, Side::Added),
            ddu_diff::ChangeKind::Removed => (colors.1, Side::Removed),
        };
        let row = mark.row as usize;
        bars.push(
            div()
                .id(("overview-mark", row))
                .absolute()
                // Additions take the left half, removals the right: a
                // replacement's two runs are a row apart, so in one
                // column the second mark covered the first.
                .map(|el| match side {
                    Side::Added => el.left_0(),
                    Side::Removed => el.right_0(),
                })
                .w(px(OVERVIEW_COLUMN))
                // A mark's own height is a share of the content, but a
                // one-line run in a long file would be a fraction of a
                // pixel: the floor is what keeps it visible (and
                // clickable) at all.
                .top(relative(top / total))
                .h(relative(height / total))
                .min_h(px(scaled(5.)))
                .bg(color)
                .cursor_pointer()
                .role(Role::Button)
                .aria_label(
                    match mark.kind {
                        ddu_diff::ChangeKind::Added => "Added lines",
                        ddu_diff::ChangeKind::Removed => "Removed lines",
                    }
                    .to_string(),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.diff_hunks_scroll
                        .scroll_to_item(row, ScrollStrategy::Top);
                    cx.notify();
                }))
                .into_any_element(),
        );
        top += height;
        cursor += mark.rows as usize;
    }
    // No viewport band: the pane's own scrollbar is in this very column
    // and already says where the viewport is — a second, worse one is
    // noise. The marks are the whole content.
    div()
        .debug_selector(|| "diff-overview".into())
        .absolute()
        // The scrollbar's own rect: gpui-base's thumb is `THUMB_WIDTH`
        // (6px) wide, `THUMB_INSET` (4px) in from the right edge, and
        // **unscaled** — so the strip is too, or it drifts out of the
        // scrollbar's column as the desktop text scale moves.
        .right(px(OVERVIEW_INSET))
        .w(px(OVERVIEW_COLUMN * 2.))
        // The rows' own padding: the strip spans the content viewport,
        // not the host box.
        .top(px(8.))
        .bottom(px(8.))
        .overflow_hidden()
        .children(bars)
        .into_any_element()
}

/// Which half of the strip a mark sits in.
#[derive(Clone, Copy)]
pub(super) enum Side {
    Added,
    Removed,
}

/// The overview's geometry, in the scrollbar's own numbers (gpui-base's
/// `THUMB_WIDTH` / `THUMB_INSET`): a 4px inset from the pane's right edge and
/// a 3px column per colour, so the strip occupies exactly the scrollbar
/// thumb's resting rect. Unscaled on purpose — the scrollbar is.
pub(super) const OVERVIEW_INSET: f32 = 4.;
pub(super) const OVERVIEW_COLUMN: f32 = 3.;
