//! Highlighted, selectable text for the pane's rows.
//!
//! `gpui-base`'s `SelectableText` lays its text out with the runs *it*
//! built from that text, so a syntax-highlighted line — one `TextRun` per
//! token — cannot ride it, and the pane's rows must stay one selectable
//! participant each (drag selection copies them joined by newlines, and
//! the find bar scrolls to a row index). This is that element with the
//! runs supplied by the caller: layout, selection registration and the
//! copy path mirror `gpui-base/src/selectable_text.rs`, which is the
//! contract window-scoped selection depends on. It rides the same
//! `TextSelectionLayer` the rest of the app already renders.
//!
//! One simplification is deliberate: a row never wraps (`whitespace:
//! nowrap`, the virtual list owns the height), so selection paints one
//! quad between the two positions instead of gpui-base's three-case
//! wrapped-line walk.

use gpui_kit::gpui::{
    App, BorderStyle, Bounds, Corners, Edges, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point,
    SharedString, StyledText, TextRun, TextStyleRefinement, Window, transparent_black,
};
use gpui_kit::base::{
    TextSelection, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun,
};

/// A line of text laid out with explicit runs, participating in the
/// window's text selection.
pub(crate) struct CodeText {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
    document_order: u64,
    style: TextStyleRefinement,
    selection_color: Option<Hsla>,
}

impl CodeText {
    pub(crate) fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        runs: Vec<TextRun>,
        style: TextStyleRefinement,
    ) -> Self {
        let text = text.into();
        Self {
            id: id.into(),
            styled: StyledText::new(text.clone()).with_runs(runs),
            text,
            document_order: 0,
            style,
            selection_color: None,
        }
    }

    /// Places this run in reading order among the others in the document:
    /// what a drag across rows copies, and in which order.
    pub(crate) fn document_order(mut self, order: u64) -> Self {
        self.document_order = order;
        self
    }
}

impl IntoElement for CodeText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CodeText {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let handle = self.handle(global_id, window, cx);
        let (layout_id, ()) = window.with_text_style(Some(self.style.clone()), |window| {
            self.styled
                .request_layout(global_id, inspector_id, window, cx)
        });
        (layout_id, handle)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        handle.register(
            TextSelectionRegistration::new(hitbox.clone(), bounds)
                .with_document_order(self.document_order)
                .with_text_bounds(vec![bounds]),
            window,
            cx,
        );
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        let before = TextSelection::selected_text(window, cx);
        let projection = handle.update_runs(
            &[
                TextSelectionRun::new(self.text.clone(), layout.clone(), bounds)
                    .with_document_order(self.document_order),
            ],
            cx,
        );
        if before != TextSelection::selected_text(window, cx) {
            window.refresh();
        }
        let color = self
            .selection_color
            .unwrap_or_else(|| gpui_kit::base::Theme::global(cx).tokens.colors.selection);
        for range in projection.ranges().iter().flatten().cloned() {
            paint_selection(&layout, range, color, window);
        }
        self.styled
            .paint(global_id, inspector_id, bounds, &mut (), &mut (), window, cx);
    }
}

impl CodeText {
    /// The selection handle, kept across frames under the element's id so
    /// a selection that started on this row survives a repaint.
    fn handle(
        &self,
        global_id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> TextSelectionHandle {
        window.with_element_state(
            global_id.expect("CodeText must have a stable element id"),
            |retained: Option<TextSelectionHandle>, _| {
                let handle =
                    retained.unwrap_or_else(|| TextSelectionHandle::new(self.text.clone(), cx));
                (handle.clone(), handle)
            },
        )
    }
}

/// The selection wash for one range of a single-line layout.
fn paint_selection(
    layout: &gpui_kit::gpui::TextLayout,
    range: std::ops::Range<usize>,
    color: Hsla,
    window: &mut Window,
) {
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return;
    };
    window.paint_quad(PaintQuad {
        bounds: Bounds::from_corners(start, Point::new(end.x, end.y + layout.line_height())),
        background: color.into(),
        corner_radii: Corners::default(),
        border_widths: Edges::default(),
        border_color: transparent_black(),
        border_style: BorderStyle::default(),
    });
}
