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

    fn render(&self, node: &MarkdownNode, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(block) = node.data::<ImageBlock>() else {
            return div().into_any_element();
        };
        v_flex()
            .w_full()
            .gap_2()
            .children(block.parts.iter().enumerate().map(|(ix, part)| match part {
                Part::Text(text) => image_run(text, ix),
                Part::Image { url, alt } => image(url, alt, &self.base, cx),
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
        .into_any_element()
}

/// One image: a filesystem path when the URL names one next to the
/// document, otherwise the URL itself (http(s) goes to gpui's own
/// loader). A local path that is not there renders its alt text, so a
/// broken reference is visible instead of silent.
fn image(url: &str, alt: &str, base: &Path, cx: &App) -> AnyElement {
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
    div()
        .w_full()
        .child(
            img(src)
                .w_full()
                .max_h(px(image_max_h()))
                .object_fit(ObjectFit::Contain),
        )
        .into_any_element()
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
    use super::{ImageBlock, Part, html_images, local_path, mdast, parse_block};
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
}
