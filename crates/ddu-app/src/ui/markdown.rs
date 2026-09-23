//! Images in a rendered Markdown document.
//!
//! gpui's text view parses `![alt](url)` and `<img src="…">` into image
//! nodes, but it hands every one of them to the app's **http** client
//! (`ImageSource::Resource(Resource::Uri)`), which cannot read a file —
//! so a README's screenshots rendered as nothing at all. This plugin
//! takes over the *block* that holds images (a paragraph, or an HTML
//! block) and renders it here: the prose through the same Markdown view
//! that would have rendered it, the image through [`img`] with a real
//! filesystem path (`Resource::Path`), resolved against the document's
//! own directory.
//!
//! A block plugin is the only hook this text view offers — inline custom
//! nodes are explicitly unsupported — which is why one *paragraph* is
//! the unit here and why its formatting is re-rendered as a nested view
//! rather than edited in place.
//!
//! A picture is painted inside the `img`'s own box, and gpui sizes a
//! relative-width `img` from the picture's intrinsic height — so a box
//! that is not capped is the pane's width by the picture's pixels, and
//! the picture painted in it is whatever fits *that* rectangle: a
//! document's screenshot came out upscaled as wide as the pane (blurry)
//! and a tall one reached past the block that reserved it, over the
//! prose below. [`image`] caps both axes; the test below pins it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::*;
use gpui_kit::*;
use gpui_kit::component::text::{
    MarkdownNode, MarkdownParseContext, MarkdownPlugin, TextView, markdown_ast as mdast,
};

/// How far down an oversized image's box may reach before the pane
/// starts scrolling the document instead: the image inside is drawn with
/// `ObjectFit::Contain`, so it is never distorted — the box is just
/// taller than the picture.
fn image_max_h() -> f32 {
    crate::ui::scaled(360.)
}

/// One piece of a block: a run of prose, or an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Image { url: String, alt: String },
}

/// The parsed content of one image-bearing block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageBlock {
    pub parts: Vec<Part>,
}

impl ImageBlock {
    /// The block's plain text, for copy and selection.
    fn plain(&self) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Image { alt, .. } => out.push_str(alt),
            }
        }
        out
    }
}

/// The plugin, carrying the directory the document lives in (a URL in
/// the source is relative to the *document*, not to the process).
pub struct LocalImages {
    /// The plugin travels to the parser's background task, so the base is
    /// shared across threads (`Arc`, not `Rc`).
    base: Arc<PathBuf>,
}

impl LocalImages {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self {
            base: Arc::new(base.into()),
        }
    }
}

impl MarkdownPlugin for LocalImages {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "ddu-images"
    }

    fn parse(&self, node: &mdast::Node, _cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let block = parse_block(node)?;
        Some(
            MarkdownNode::new("ddu-images", block.clone())
                .text(block.plain())
                .markdown(markdown_of(&block)),
        )
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(block) = node.data::<ImageBlock>() else {
            return div().into_any_element();
        };
        v_flex()
            .w_full()
            .gap_2()
            .children(block.parts.iter().enumerate().map(|(ix, part)| match part {
                Part::Text(text) => image_run(text, ix),
                Part::Image { url, alt } => image(url, alt, &self.base, ix, window, cx),
            }))
            .into_any_element()
    }
}

/// The block this plugin takes over, if `node` is one: a paragraph
/// holding an image, or a raw HTML block that embeds one. Everything
/// else is left to the text view.
fn parse_block(node: &mdast::Node) -> Option<ImageBlock> {
    {
        let block = match node {
            mdast::Node::Paragraph(paragraph) => {
                let mut block = ImageBlock::default();
                let mut text = String::new();
                for child in &paragraph.children {
                    collect(child, &mut block, &mut text);
                }
                flush(&mut block, &mut text);
                // Not our paragraph unless an image turned up.
                block.parts.iter().any(|p| matches!(p, Part::Image { .. })).then_some(block)?
            }
            mdast::Node::Html(html) => {
                let parts = html_images(&html.value);
                if parts.is_empty() {
                    return None;
                }
                ImageBlock { parts }
            }
            _ => return None,
        };
        Some(block)
    }
}

/// A prose run beside an image: the same Markdown view the paragraph
/// would have been rendered by, so the two cannot drift apart in size,
/// spacing or inline formatting (bold, links, `code`).
fn image_run(text: &str, ix: usize) -> AnyElement {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let id = format!("md-image-run-{ix}-{:x}", hasher.finish());
    TextView::markdown(id, text.to_owned())
        .selectable(false)
        .scrollable(false)
        // The paragraph's own style, size and heading scale included: a
        // run rendered beside an image must not drift from the one the
        // document body renders (see [`super::document_text_style`]).
        .style(super::document_text_style())
        .into_any_element()
}

/// One image: a filesystem path when the URL names one next to the
/// document, otherwise the URL taken as a path — a remote `http(s)` image
/// is read as a file with that name and renders nothing, since the loader
/// the text view would have handed it to is the app's http client and
/// [`img`] here takes a path. A local path that is not there renders its
/// alt text, so a broken reference is visible instead of silent.
///
/// The caps are on the `img` itself, and that is load-bearing: the `img` is
/// what paints, so the box taffy sizes for it is the rectangle
/// `ObjectFit::Contain` fits the picture into, and the block holding it
/// reserves exactly that rectangle. An *uncapped* `w_full` box is the pane's
/// width by the picture's own pixels — gpui gives a relative-width picture
/// its intrinsic height — while the picture is fitted to that box: measured
/// on the pane that rendered `contrib/usage/README.md` (812px of room, a
/// 250×52 `menubar.png`), an 812×169 blur out of a 812×52 box, 117px over
/// the paragraph the block placed 8px below.
///
/// A wrapper must not carry the caps instead. A `100%`-height child of the
/// box resolves against the ratio height *before* the caps are applied, so a
/// wrapper that capped its own width reserved 688×480 while the picture
/// inside it painted its natural 688×652 — 172px over the prose below, on
/// the 977px pane this was reported from (the width cap is the trigger: at a
/// pane narrower than the picture the two agree).
///
/// The size itself comes from the loaded asset (`use_asset` — the cache
/// [`img`] loads through; a `Resource::Path` decodes at scale factor 1, so
/// its pixel size is the size gpui paints it at): the width is capped at the
/// picture's own, so a picture narrower than the pane is drawn at the size
/// it was made at (never blown up), and the height is capped at
/// [`image_max_h`], so a tall picture is as tall as the pane allows with the
/// picture letterboxed inside the box. Both caps are needed — taffy clamps
/// each axis on its own, and one cap alone leaves the other axis free
/// (measured: a 250×52 picture in a 500px pane boxed 250×104 with only the
/// width cap).
fn image(
    url: &str,
    alt: &str,
    base: &Path,
    ix: usize,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let local = local_path(base, url);
    let src = match &local {
        Some(path) => path.clone(),
        None => PathBuf::from(url),
    };
    let missing = local.is_some() && !src.exists();
    if missing {
        if alt.is_empty() {
            return div().into_any_element();
        }
        return crate::ui::meta_text(alt.to_owned(), cx).into_any_element();
    }
    let Some(natural) = natural_size(&src, window, cx) else {
        // Not measured yet — `use_asset` has already arranged for this view
        // to be drawn again when the picture lands, so this is one frame:
        // sized by the picture's own, never upscaled, so the box cannot
        // reach past the picture even then. The `img` alone, like the
        // measured case: a wrapper is what this function exists to avoid.
        return img(src)
            .max_w_full()
            .max_h(px(image_max_h()))
            .object_fit(ObjectFit::ScaleDown)
            .into_any_element();
    };
    // A picture is never blown up past the size it was made at, and never
    // taller than the pane shows. The element that carries those caps is
    // the `img` itself: it is what paints, so the box that is sized here
    // is the box it fits the picture into, and the block that holds it
    // reserves exactly the rectangle the picture can reach.
    let tall = image_max_h().min(f32::from(natural.height));
    img(src)
        .w_full()
        .max_w(natural.width)
        .max_h(px(tall))
        .object_fit(ObjectFit::Contain)
        // The test hook: the box the picture is painted in.
        .debug_selector(move || format!("md-image-{ix}"))
        .into_any_element()
}

/// The picture's own size, straight from the asset cache [`img`] loads it
/// through — `None` until that load lands, and for a file gpui cannot
/// decode at all (a broken picture, an unreadable path). A path resource
/// decodes at scale factor 1, so its pixel size is the size gpui paints
/// it at.
fn natural_size(
    src: &Path,
    window: &mut Window,
    cx: &mut App,
) -> Option<gpui_kit::Size<Pixels>> {
    let resource = Resource::from(src.to_path_buf());
    let data = window.use_asset::<ImgResourceLoader>(&resource, cx)?.ok()?;
    let size = data.size(0).map(|d| px(u32::from(d) as f32));
    (size.width > px(0.) && size.height > px(0.)).then_some(size)
}

/// The filesystem path a URL names, when it names one: a scheme
/// (`http:`, `https:`, `data:`) is left to gpui's loader, and so is an
/// absolute path that starts with `/` on another root.
fn local_path(base: &Path, url: &str) -> Option<PathBuf> {
    if url.is_empty() || url.starts_with('#') {
        return None;
    }
    if let Some(scheme) = url.find(':') {
        let scheme_ok = url[..scheme]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.');
        // A Windows drive letter is not a scheme, but this app's images
        // never are one either — treat any `scheme:` as remote.
        if scheme_ok && scheme > 0 {
            return None;
        }
    }
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let decoded = percent_decode(url);
    let path = Path::new(&decoded);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    Some(base.join(path))
}

/// `%20` and friends, so a path with a space resolves.
fn percent_decode(url: &str) -> String {
    let bytes = url.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] == b'%' && ix + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[ix + 1..ix + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                ix += 3;
                continue;
            }
        }
        out.push(bytes[ix]);
        ix += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Walk an inline node: prose accumulates into `text`, an image flushes
/// the run and lands in the block. Inline markup (emphasis, links) keeps
/// its text — the run is re-rendered as Markdown below.
fn collect(node: &mdast::Node, block: &mut ImageBlock, text: &mut String) {
    match node {
        mdast::Node::Image(image) => {
            flush(block, text);
            block.parts.push(Part::Image {
                url: image.url.clone(),
                alt: image.alt.clone(),
            });
        }
        mdast::Node::Text(value) => text.push_str(&value.value),
        mdast::Node::InlineCode(code) => {
            text.push('`');
            text.push_str(&code.value);
            text.push('`');
        }
        mdast::Node::Break(_) => text.push('\n'),
        // Nothing to render for a rule/footnote reference inside a
        // paragraph; the rest is markup whose text we keep.
        _ => {
            if let Some(children) = node.children() {
                for child in children {
                    collect(child, block, text);
                }
            }
        }
    }
}

fn flush(block: &mut ImageBlock, text: &mut String) {
    let run = std::mem::take(text);
    if !run.trim().is_empty() {
        block.parts.push(Part::Text(run));
    }
}

/// Images inside a raw HTML block: `<img src="…" alt="…" width="…">`,
/// which is how most READMEs (this repository's included) embed a
/// screenshot. Anything else in the block is markup — there is nothing
/// visible to keep.
fn html_images(raw: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let lower = raw.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(start) = rest.find("<img") {
        rest = &rest[start + 4..];
        let Some(end) = rest.find('>') else { break };
        // Attributes come off the original text: the lowercase copy is
        // only a search aid, and a URL's case matters.
        let offset = raw.len() - rest.len();
        let original = &raw[offset..offset + end];
        let url = attr(original, "src").unwrap_or_default();
        let alt = attr(original, "alt").unwrap_or_default();
        if !url.is_empty() {
            parts.push(Part::Image { url, alt });
        }
        rest = &rest[end..];
    }
    parts
}

/// One HTML attribute's value, quoted or bare.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let ix = from + pos;
        from = ix + name.len();
        // The attribute must stand alone (`src`, not `data-src`).
        let before_ok = ix == 0
            || !tag.as_bytes()[ix - 1].is_ascii_alphanumeric() && tag.as_bytes()[ix - 1] != b'-';
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        if !before_ok {
            continue;
        }
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or_default().to_owned(),
            _ => rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned(),
        };
        return Some(value);
    }
    None
}

/// The block as Markdown source, for copy-selection.
fn markdown_of(block: &ImageBlock) -> String {
    block
        .parts
        .iter()
        .map(|part| match part {
            Part::Text(text) => text.clone(),
            Part::Image { url, alt } => format!("![{alt}]({url})"),
        })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    // Explicit, not `use super::*`: the parent's glob imports would drag
    // in every component item, and `#[test]` must stay the test harness's.
    use super::{ImageBlock, Part, image_max_h, local_path, mdast, parse_block};
    use gpui_kit::{
        Bounds, DevicePixels, Entity, ObjectFit, Pixels, Render, SharedString, px, size,
    };
    use std::path::{Path, PathBuf};
    // Aliased: the component's own `Text` is in scope through its glob
    // import, and the two are not interchangeable.
    use mdast::{Image as MdImage, Node, Paragraph as MdParagraph, Text as MdText};

    fn text(value: &str) -> Node {
        Node::Text(MdText {
            value: value.to_owned(),
            position: None,
        })
    }

    fn image(url: &str, alt: &str) -> Node {
        Node::Image(MdImage {
            url: url.to_owned(),
            alt: alt.to_owned(),
            title: None,
            position: None,
        })
    }

    fn paragraph(children: Vec<Node>) -> Node {
        Node::Paragraph(MdParagraph {
            children,
            position: None,
        })
    }

    fn parse(node: &Node) -> Option<ImageBlock> {
        parse_block(node)
    }

    /// Prose and pictures keep their order, and the block reports the
    /// text a copy would take.
    #[test]
    fn a_paragraph_splits_into_runs_and_images() {
        let node = paragraph(vec![
            text("before "),
            image("shot.png", "a screenshot"),
            text(" after"),
        ]);
        let block = parse(&node).expect("an image block");
        assert_eq!(
            block.parts,
            [
                Part::Text("before ".to_owned()),
                Part::Image {
                    url: "shot.png".to_owned(),
                    alt: "a screenshot".to_owned(),
                },
                Part::Text(" after".to_owned()),
            ]
        );
        assert_eq!(block.plain(), "before a screenshot after");
    }

    /// A paragraph without an image is left to the text view, and so is
    /// an HTML block that carries none.
    #[test]
    fn only_image_blocks_are_taken_over() {
        assert!(parse(&paragraph(vec![text("just prose")])).is_none());
        assert!(parse(&Node::Html(mdast::Html {
            value: "<p>plain</p>".to_owned(),
            position: None
        }))
        .is_none());
    }

    /// Raw HTML: the common README form, including the `width` attribute
    /// the tag may carry and a single-quoted value.
    #[test]
    fn html_images_come_out_of_the_tag() {
        let html = Node::Html(mdast::Html {
            value: "<p align=\"center\">\n<img src=\"assets/main.png\" alt=\"shot\" width=\"960\">\n</p>"
                .to_owned(),
            position: None,
        });
        let block = parse(&html).expect("an image block");
        assert_eq!(
            block.parts,
            [Part::Image {
                url: "assets/main.png".to_owned(),
                alt: "shot".to_owned(),
            }]
        );

        let single = parse(&Node::Html(mdast::Html {
            value: "<img src='a b.png'>".to_owned(),
            position: None,
        }))
        .expect("an image block");
        assert_eq!(
            single.parts,
            [Part::Image {
                url: "a b.png".to_owned(),
                alt: String::new(),
            }]
        );
    }

    /// URL handling: a relative path resolves against the document,
    /// schemes stay with gpui's loader, and query/escape noise is
    /// stripped.
    #[test]
    fn urls_resolve_against_the_document() {
        let base = Path::new("/repo/docs");
        assert_eq!(
            local_path(base, "img/shot.png"),
            Some(PathBuf::from("/repo/docs/img/shot.png"))
        );
        assert_eq!(
            local_path(base, "./shot.png"),
            Some(PathBuf::from("/repo/docs/./shot.png"))
        );
        assert_eq!(local_path(base, "/abs/shot.png"), Some(PathBuf::from("/abs/shot.png")));
        assert_eq!(
            local_path(base, "shot%20one.png"),
            Some(PathBuf::from("/repo/docs/shot one.png"))
        );
        assert_eq!(
            local_path(base, "shot.png?raw=true"),
            Some(PathBuf::from("/repo/docs/shot.png"))
        );
        assert_eq!(local_path(base, "https://example.com/a.png"), None);
        assert_eq!(local_path(base, "data:image/png;base64,AAAA"), None);
        assert_eq!(local_path(base, ""), None);
    }

    /// The box a document's picture is painted in holds the picture: the
    /// pane's width or the picture's own, whichever is narrower, and the
    /// picture's height or the pane's cap, whichever is shorter — and the
    /// picture inside is fitted to that box, its own ratio kept. The prose
    /// the block places below the box therefore clears the picture.
    ///
    /// The bug this pins is the painted picture leaving the reserved box.
    /// `img` takes the height of a *relative*-width picture from the
    /// picture's intrinsic height while `Contain` fits the picture to the
    /// box's *width*: an uncapped 250x52 `menubar.png` in the 812px pane
    /// that rendered `contrib/usage/README.md` was boxed 812x52 and
    /// painted 812x169, 117px over the paragraph the block placed 8px
    /// below. Capping only the width leaves the mirror image — a 250x52
    /// picture in a 500px pane boxed 250x104 — and capping only the height
    /// leaves a tall picture's box at the pane's width, 100s of px of
    /// blank under a narrow picture. The cases below are those rectangles
    /// — including the 977px pane `panel.png` (688x652, cap 360) was
    /// reported from, where the picture is *wider* than the pane: that is
    /// the width cap's case, and where a box that caps itself reserves less
    /// than the picture inside it paints.
    #[gpui_kit::gpui::test]
    async fn a_picture_is_boxed_at_the_size_it_is_painted(cx: &mut gpui_kit::TestAppContext) {
        use super::{LocalImages, TextView};
        use gpui_kit::prelude::*;
        use gpui_kit::{Context, Render, Window, div};
        cx.update(gpui_kit::init);

        struct Doc {
            width: Pixels,
            source: String,
            dir: PathBuf,
        }
        impl Render for Doc {
            fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(self.width)
                    .child(
                        TextView::markdown(
                            SharedString::from(format!("md-box-{}", self.width)),
                            self.source.clone(),
                        )
                        .scrollable(false)
                        .selectable(false)
                        .style(crate::ui::document_text_style())
                        .plugin(LocalImages::new(self.dir.clone())),
                    )
                    .into_any_element()
            }
        }

        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contrib/usage/docs");
        // The pictures this repository ships are the two shapes that show
        // the bug: a 250x52 menu-bar strip, narrower than any pane, and a
        // 688x652 panel shot, wider than the pane and taller than its cap.
        for (file, pane) in [
            ("menubar.png", px(500.)),
            ("menubar.png", px(200.)),
            ("panel.png", px(500.)),
            ("panel.png", px(977.)),
        ] {
            let source = format!("![shot]({file})\n\nProse under the picture.\n");
            let (view, vcx) = cx.add_window_view(|_, _| Doc {
                width: pane,
                source: String::new(),
                dir: dir.clone(),
            });
            vcx.update(|_, cx| {
                view.update(cx, |doc, cx| {
                    doc.source = source.clone();
                    cx.notify();
                })
            });
            let box_ = load_picture(vcx, &view, "md-image-0");
            let natural = png_size(&dir.join(file));
            let expected = size(
                natural.width.min(pane),
                natural.height.min(px(image_max_h())),
            );
            assert!(
                (box_.size.width - expected.width).abs() < px(0.5)
                    && (box_.size.height - expected.height).abs() < px(0.5),
                "{file} ({natural:?}) in a {pane:?} pane is boxed {box_:?}, \
                 not the {expected:?} the pane's room and the caps allow"
            );

            // What `img` paints inside that box: gpui's own fit, so the
            // assertion is about the box the picture is given, not about a
            // second copy of the ratio math.
            let painted = ObjectFit::Contain.get_bounds(
                box_,
                size(
                    DevicePixels::from(u32::from(natural.width)),
                    DevicePixels::from(u32::from(natural.height)),
                ),
            );
            let ratio = f32::from(natural.width) / f32::from(natural.height);
            assert!(
                painted.size.width <= box_.size.width + px(0.5)
                    && painted.size.height <= box_.size.height + px(0.5),
                "{file} ({natural:?}) is painted {painted:?} out of its {box_:?} box"
            );
            assert!(
                painted.size.width <= natural.width + px(0.5)
                    && painted.size.height <= natural.height + px(0.5)
                    && (painted.size.width / painted.size.height - ratio).abs() < 0.001,
                "{file} ({natural:?}) is painted {painted:?}: stretched, or blown \
                 up past the size it was made at"
            );
        }
    }

    /// The size a PNG's own header reports, so the test's expectation is
    /// the file's and not a number retyped here.
    fn png_size(path: &Path) -> gpui_kit::Size<Pixels> {
        let bytes = std::fs::read(path).expect("the picture this test boxes");
        let width = u32::from_be_bytes(bytes[16..20].try_into().expect("a PNG header"));
        let height = u32::from_be_bytes(bytes[20..24].try_into().expect("a PNG header"));
        size(px(width as f32), px(height as f32))
    }

    /// Draw until the picture's box is measured: the asset cache reads and
    /// decodes the file off the executor and redraws the view when it lands,
    /// so the first frames carry an unmeasured block.
    fn load_picture<V: Render>(
        vcx: &mut gpui_kit::VisualTestContext,
        view: &Entity<V>,
        selector: &'static str,
    ) -> Bounds<Pixels> {
        for _ in 0..200 {
            vcx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            vcx.run_until_parked();
            if let Some(bounds) = vcx.debug_bounds(selector) {
                return bounds;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the picture's box was never measured");
    }
}
