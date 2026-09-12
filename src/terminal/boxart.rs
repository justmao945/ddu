//! Vector drawing for box-drawing (U+2500–U+257F) and block-element
//! (U+2580–U+259F) characters.
//!
//! Font glyphs for these sit on the font's own bounding box, so at our
//! 1.45 line height they leave vertical gaps between rows — TUI panels
//! and block cursors/bars fall apart. Drawing them as rectangles
//! against the cell box makes every stroke seamless by construction,
//! like alacritty's builtin box-drawing font.
//!
//! Only the diagonal glyphs fall back to the font (returned as
//! non-vector): every other char in the block is drawn as cell-aligned
//! rects, so a table's crossings cannot disagree with its borders.

use gpui_kit::*;

/// Stroke weight/style of one cell edge arm.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stroke {
    None,
    Light,
    Heavy,
    Double,
    /// Two dashes along the arm (╌ family).
    Dash2,
    /// Three dashes along the arm (┄ family).
    Dash3,
    /// Four dashes along the arm (┈ family).
    Dash4,
}

/// Arms in [left, right, up, down] order.
type Arms = [Stroke; 4];

const N: Stroke = Stroke::None;
const L: Stroke = Stroke::Light;
const H: Stroke = Stroke::Heavy;
const D: Stroke = Stroke::Double;

/// True when `c` gets vector drawing instead of a font glyph.
pub(crate) fn is_vector(c: char) -> bool {
    match c {
        '\u{2500}'..='\u{257F}' => arms(c).is_some() || is_arc(c),
        '\u{2580}'..='\u{259F}' => true,
        _ => false,
    }
}

/// Paint `c` into `bounds` using `color` (the cell's foreground).
pub(crate) fn paint(c: char, bounds: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    let x0 = f32::from(bounds.origin.x);
    let y0 = f32::from(bounds.origin.y);
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);

    // Blocks first: eighth fills, quadrants, shades.
    if ('\u{2580}'..='\u{259F}').contains(&c) {
        paint_block(c, x0, y0, w, h, color, window);
        return;
    }
    if is_arc(c) {
        paint_arc(c, x0, y0, w, h, color, window);
        return;
    }
    let Some(arms) = arms(c) else { return };

    let t = (w.min(h) / 10.).round().max(1.);
    let cx = x0 + w / 2.;
    let cy = y0 + h / 2.;
    // Arms run edge → slightly past center so perpendicular strokes
    // butt-joint without a pinhole.
    let rects = [
        (arms[0], (x0, cx + t / 2.), Axis::H, cy),
        (arms[1], (cx - t / 2., x0 + w), Axis::H, cy),
        (arms[2], (y0, cy + t / 2.), Axis::V, cx),
        (arms[3], (cy - t / 2., y0 + h), Axis::V, cx),
    ];
    for (stroke, (a, b), axis, center) in rects {
        paint_arm(stroke, axis, a, b, center, t, color, window);
    }
}

#[derive(Clone, Copy)]
enum Axis {
    H,
    V,
}

/// One arm: solid/heavy/double/dashed rect(s) along its axis.
fn paint_arm(
    stroke: Stroke,
    axis: Axis,
    from: f32,
    to: f32,
    center: f32,
    t: f32,
    color: Hsla,
    window: &mut Window,
) {
    if stroke == N {
        return;
    }
    let rect = |along_a: f32, along_b: f32, cross_a: f32, cross_b: f32| match axis {
        Axis::H => Bounds {
            origin: point(px(along_a), px(cross_a)),
            size: size(px((along_b - along_a).max(0.)), px(cross_b - cross_a)),
        },
        Axis::V => Bounds {
            origin: point(px(cross_a), px(along_a)),
            size: size(px(cross_b - cross_a), px((along_b - along_a).max(0.))),
        },
    };
    match stroke {
        N => {}
        L => window.paint_quad(fill(
            rect(from, to, center - t / 2., center + t / 2.),
            color,
        )),
        H => window.paint_quad(fill(rect(from, to, center - t, center + t), color)),
        D => {
            // Two light strokes, one thickness apart around the center.
            window.paint_quad(fill(
                rect(from, to, center - 1.5 * t, center - 0.5 * t),
                color,
            ));
            window.paint_quad(fill(
                rect(from, to, center + 0.5 * t, center + 1.5 * t),
                color,
            ));
        }
        Stroke::Dash2 | Stroke::Dash3 | Stroke::Dash4 => {
            let dashes = match stroke {
                Stroke::Dash2 => 2,
                Stroke::Dash3 => 3,
                _ => 4,
            };
            let step = (to - from) / dashes as f32;
            let gap = step * 0.3;
            for i in 0..dashes {
                let a = from + step * i as f32 + gap / 2.;
                let b = from + step * (i + 1) as f32 - gap / 2.;
                window.paint_quad(fill(rect(a, b, center - t / 2., center + t / 2.), color));
            }
        }
    }
}

/// Rounded corners ╭╮╰╯: a transparent quad stroked on the two
/// connected sides of one half-cell, the corner radius curving them.
fn paint_arc(c: char, x0: f32, y0: f32, w: f32, h: f32, color: Hsla, window: &mut Window) {
    let t = (w.min(h) / 10.).round().max(1.);
    let cx = x0 + w / 2.;
    let cy = y0 + h / 2.;
    let r = (w / 2.).min(h / 2.);
    // Stroke lines sit on the cell centerlines: shrink the arc quad by
    // t/2 so the border's center lands there.
    let (origin, sz, radii, edges) = match c {
        // ╭: right + down arms; quad spans center → bottom-right.
        '\u{256D}' => (
            point(px(cx - t / 2.), px(cy - t / 2.)),
            size(px(w / 2. + t / 2.), px(h / 2. + t / 2.)),
            Corners {
                top_left: px(r),
                ..Default::default()
            },
            Edges {
                top: px(t),
                left: px(t),
                ..Default::default()
            },
        ),
        // ╮: left + down; quad spans left → center, center → bottom.
        '\u{256E}' => (
            point(px(x0 - t / 2.), px(cy - t / 2.)),
            size(px(w / 2. + t), px(h / 2. + t / 2.)),
            Corners {
                top_right: px(r),
                ..Default::default()
            },
            Edges {
                top: px(t),
                right: px(t),
                ..Default::default()
            },
        ),
        // ╯: left + up; quad spans left → center, top → center.
        '\u{256F}' => (
            point(px(x0 - t / 2.), px(y0 - t / 2.)),
            size(px(w / 2. + t), px(h / 2. + t)),
            Corners {
                bottom_right: px(r),
                ..Default::default()
            },
            Edges {
                right: px(t),
                bottom: px(t),
                ..Default::default()
            },
        ),
        // ╰: right + up; quad spans center → right, top → center.
        '\u{2570}' => (
            point(px(cx - t / 2.), px(y0 - t / 2.)),
            size(px(w / 2. + t / 2.), px(h / 2. + t)),
            Corners {
                bottom_left: px(r),
                ..Default::default()
            },
            Edges {
                left: px(t),
                bottom: px(t),
                ..Default::default()
            },
        ),
        _ => return,
    };
    window.paint_quad(PaintQuad {
        bounds: Bounds { origin, size: sz },
        corner_radii: radii,
        background: transparent_black().into(),
        border_widths: edges,
        border_color: color,
        border_style: BorderStyle::Solid,
    });
}

fn is_arc(c: char) -> bool {
    ('\u{256D}'..='\u{2570}').contains(&c)
}

/// Block elements: eighth fills, half/quarter cells, shades.
fn paint_block(c: char, x0: f32, y0: f32, w: f32, h: f32, color: Hsla, window: &mut Window) {
    match c {
        '\u{2580}' => block_v(x0, y0, w, h, 0., 0.5, color, window), // ▀ upper half
        '\u{2581}'..='\u{2587}' => {
            // ▁▂▃▄▅▆▇ lower k/8
            let k = c as u32 - '\u{2580}' as u32;
            block_v(x0, y0, w, h, 1. - k as f32 / 8., 1., color, window)
        }
        '\u{2588}' => block_v(x0, y0, w, h, 0., 1., color, window), // █ full
        '\u{2589}'..='\u{258F}' => {
            // ▉▊▋▌▍▎▏ left (8-k)/8 … 1/8
            let k = c as u32 - '\u{2588}' as u32;
            block_h(x0, y0, w, h, 0., (8 - k) as f32 / 8., color, window)
        }
        '\u{2590}' => block_h(x0, y0, w, h, 0.5, 1., color, window), // ▐ right half
        // Shades: solid fill at reduced alpha (over any cell bg).
        '\u{2591}' => vrect_alpha(x0, y0, w, h, color, 0.25, window), // ░
        '\u{2592}' => vrect_alpha(x0, y0, w, h, color, 0.5, window),  // ▒
        '\u{2593}' => vrect_alpha(x0, y0, w, h, color, 0.75, window), // ▓
        '\u{2594}' => block_v(x0, y0, w, h, 0., 0.125, color, window), // ▔ upper 1/8
        '\u{2595}' => block_h(x0, y0, w, h, 0.875, 1., color, window), // ▕ right 1/8
        '\u{2596}' => block_quads(x0, y0, w, h, 0b0100, color, window), // ▖
        '\u{2597}' => block_quads(x0, y0, w, h, 0b1000, color, window), // ▗
        '\u{2598}' => block_quads(x0, y0, w, h, 0b0001, color, window), // ▘
        '\u{2599}' => block_quads(x0, y0, w, h, 0b0111, color, window), // ▙
        '\u{259A}' => block_quads(x0, y0, w, h, 0b1001, color, window), // ▚
        '\u{259B}' => block_quads(x0, y0, w, h, 0b1011, color, window), // ▛
        '\u{259C}' => block_quads(x0, y0, w, h, 0b1101, color, window), // ▜
        '\u{259D}' => block_quads(x0, y0, w, h, 0b0010, color, window), // ▝
        '\u{259E}' => block_quads(x0, y0, w, h, 0b1010, color, window), // ▞
        '\u{259F}' => block_quads(x0, y0, w, h, 0b1110, color, window), // ▟
        _ => {}
    }
}

/// Vertical fraction fill: full width, y from `ya` to `yb` (0..=1).
fn block_v(x0: f32, y0: f32, w: f32, h: f32, ya: f32, yb: f32, color: Hsla, window: &mut Window) {
    window.paint_quad(fill(
        Bounds {
            origin: point(px(x0), px(y0 + h * ya)),
            size: size(px(w), px(h * (yb - ya))),
        },
        color,
    ));
}

/// Horizontal fraction fill: full height, x from `xa` to `xb` (0..=1).
fn block_h(x0: f32, y0: f32, w: f32, h: f32, xa: f32, xb: f32, color: Hsla, window: &mut Window) {
    window.paint_quad(fill(
        Bounds {
            origin: point(px(x0 + w * xa), px(y0)),
            size: size(px(w * (xb - xa)), px(h)),
        },
        color,
    ));
}

/// Quadrant fills (half cell each): bit0=upper-left, bit1=upper-right,
/// bit2=lower-left, bit3=lower-right.
fn block_quads(x0: f32, y0: f32, w: f32, h: f32, mask: u8, color: Hsla, window: &mut Window) {
    let hw = w / 2.;
    let hh = h / 2.;
    for (bit, qx, qy) in [(1u8, 0., 0.), (2, hw, 0.), (4, 0., hh), (8, hw, hh)] {
        if mask & bit != 0 {
            window.paint_quad(fill(
                Bounds {
                    origin: point(px(x0 + qx), px(y0 + qy)),
                    size: size(px(hw), px(hh)),
                },
                color,
            ));
        }
    }
}

fn vrect_alpha(x0: f32, y0: f32, w: f32, h: f32, color: Hsla, alpha: f32, window: &mut Window) {
    window.paint_quad(fill(
        Bounds {
            origin: point(px(x0), px(y0)),
            size: size(px(w), px(h)),
        },
        color.opacity(alpha),
    ));
}

/// Arm table for the covered box-drawing chars. Anything omitted (the
/// mixed-weight tees/crosses, double-dash extras, diagonals) falls back
/// to the font glyph.
fn arms(c: char) -> Option<Arms> {
    Some(match c {
        '\u{2500}' => [L, L, N, N],                         // ─
        '\u{2501}' => [H, H, N, N],                         // ━
        '\u{2502}' => [N, N, L, L],                         // │
        '\u{2503}' => [N, N, H, H],                         // ┃
        '\u{2504}' => [Stroke::Dash3, Stroke::Dash3, N, N], // ┄
        '\u{2505}' => [H, H, N, N],                         // ┅ (heavy dash → solid heavy)
        '\u{2506}' => [N, N, Stroke::Dash3, Stroke::Dash3], // ┆
        '\u{2507}' => [N, N, H, H],                         // ┇
        '\u{2508}' => [Stroke::Dash4, Stroke::Dash4, N, N], // ┈
        '\u{2509}' => [H, H, N, N],                         // ┉
        '\u{250A}' => [N, N, Stroke::Dash4, Stroke::Dash4], // ┊
        '\u{250B}' => [N, N, H, H],                         // ┋
        '\u{250C}' => [N, L, N, L],                         // ┌
        '\u{250D}' => [N, H, N, L],                         // ┍
        '\u{250E}' => [N, L, N, H],                         // ┎
        '\u{250F}' => [N, H, N, H],                         // ┏
        '\u{2510}' => [L, N, N, L],                         // ┐
        '\u{2511}' => [H, N, N, L],                         // ┑
        '\u{2512}' => [L, N, N, H],                         // ┒
        '\u{2513}' => [H, N, N, H],                         // ┓
        '\u{2514}' => [N, L, L, N],                         // └
        '\u{2515}' => [N, H, L, N],                         // ┕
        '\u{2516}' => [N, L, H, N],                         // ┖
        '\u{2517}' => [N, H, H, N],                         // ┗
        '\u{2518}' => [L, N, L, N],                         // ┘
        '\u{2519}' => [H, N, L, N],                         // ┙
        '\u{251A}' => [L, N, H, N],                         // ┚
        '\u{251B}' => [H, N, H, N],                         // ┛
        '\u{251C}' => [N, L, L, L],                         // ├
        '\u{251D}' => [N, H, L, L],                         // ┝
        '\u{251E}' => [N, L, H, L],                         // ┞
        '\u{251F}' => [N, L, L, H],                         // ┟
        '\u{2520}' => [N, L, H, H],                         // ┠
        '\u{2521}' => [N, H, H, L],                         // ┡
        '\u{2522}' => [N, H, L, H],                         // ┢
        '\u{2523}' => [N, H, H, H],                         // ┣
        '\u{2524}' => [L, N, L, L],                         // ┤
        '\u{2525}' => [H, N, L, L],                         // ┥
        '\u{2526}' => [L, N, H, L],                         // ┦
        '\u{2527}' => [L, N, L, H],                         // ┧
        '\u{2528}' => [L, N, H, H],                         // ┨
        '\u{2529}' => [H, N, H, L],                         // ┩
        '\u{252A}' => [H, N, L, H],                         // ┪
        '\u{252B}' => [H, N, H, H],                         // ┫
        '\u{252C}' => [L, L, N, L],                         // ┬
        '\u{252D}' => [H, L, N, L],                         // ┭
        '\u{252E}' => [L, H, N, L],                         // ┮
        '\u{252F}' => [H, H, N, L],                         // ┯
        '\u{2530}' => [L, L, N, H],                         // ┰
        '\u{2531}' => [H, L, N, H],                         // ┱
        '\u{2532}' => [L, H, N, H],                         // ┲
        '\u{2533}' => [H, H, N, H],                         // ┳
        '\u{2534}' => [L, L, L, N],                         // ┴
        '\u{2535}' => [H, H, L, N],                         // ┵
        '\u{2536}' => [L, L, H, N],                         // ┶
        '\u{2537}' => [H, L, L, N],                         // ┷
        '\u{2538}' => [L, H, L, N],                         // ┸
        '\u{2539}' => [H, L, H, N],                         // ┹
        '\u{253A}' => [L, H, H, N],                         // ┺
        '\u{253B}' => [L, L, H, N],                         // ┻
        '\u{253C}' => [L, L, L, L],                         // ┼
        '\u{253D}' => [H, L, L, L],                         // ┽
        '\u{253E}' => [L, H, L, L],                         // ┾
        '\u{253F}' => [H, H, L, L],                         // ┿
        '\u{2540}' => [L, L, H, L],                         // ╀
        '\u{2541}' => [L, L, L, H],                         // ╁
        '\u{2542}' => [L, L, H, H],                         // ╂
        '\u{2543}' => [H, L, H, L],                         // ╃
        '\u{2544}' => [L, H, H, L],                         // ╄
        '\u{2545}' => [H, L, L, H],                         // ╅
        '\u{2546}' => [L, H, L, H],                         // ╆
        '\u{2547}' => [H, H, H, L],                         // ╇
        '\u{2548}' => [H, H, L, H],                         // ╈
        '\u{2549}' => [H, L, H, H],                         // ╉
        '\u{254A}' => [L, H, H, H],                         // ╊
        '\u{254B}' => [H, H, H, H],                         // ╋
        '\u{254C}' => [Stroke::Dash2, Stroke::Dash2, N, N], // ╌
        '\u{254D}' => [H, H, N, N],                         // ╍ (heavy dash → solid heavy)
        '\u{254E}' => [N, N, Stroke::Dash2, Stroke::Dash2], // ╎
        '\u{254F}' => [N, N, H, H],                         // ╏
        '\u{2550}' => [D, D, N, N],                         // ═
        '\u{2551}' => [N, N, D, D],                         // ║
        '\u{2552}' => [N, D, N, L],                         // ╒
        '\u{2553}' => [N, L, N, D],                         // ╓
        '\u{2554}' => [N, D, N, D],                         // ╔
        '\u{2555}' => [D, N, N, L],                         // ╕
        '\u{2556}' => [L, N, N, D],                         // ╖
        '\u{2557}' => [D, N, N, D],                         // ╗
        '\u{2558}' => [N, D, L, N],                         // ╘
        '\u{2559}' => [N, L, D, N],                         // ╙
        '\u{255A}' => [N, D, D, N],                         // ╚
        '\u{255B}' => [D, N, L, N],                         // ╛
        '\u{255C}' => [L, N, D, N],                         // ╜
        '\u{255D}' => [D, N, D, N],                         // ╝
        '\u{255E}' => [N, D, L, L],                         // ╞
        '\u{255F}' => [N, L, D, D],                         // ╟
        '\u{2560}' => [N, D, D, D],                         // ╠
        '\u{2561}' => [D, N, L, L],                         // ╡
        '\u{2562}' => [L, N, D, D],                         // ╢
        '\u{2563}' => [D, N, D, D],                         // ╣
        '\u{2564}' => [D, D, N, L],                         // ╤
        '\u{2565}' => [L, L, N, D],                         // ╥
        '\u{2566}' => [D, D, N, D],                         // ╦
        '\u{2567}' => [D, D, L, N],                         // ╧
        '\u{2568}' => [L, L, D, N],                         // ╨
        '\u{2569}' => [D, D, D, N],                         // ╩
        '\u{256A}' => [D, D, L, L],                         // ╪
        '\u{256B}' => [L, L, D, D],                         // ╫
        '\u{256C}' => [D, D, D, D],                         // ╬
        // Single-arm stubs.
        '\u{2574}' => [L, N, N, N], // ╴
        '\u{2575}' => [N, N, L, N], // ╵
        '\u{2576}' => [N, L, N, N], // ╶
        '\u{2577}' => [N, N, N, L], // ╷
        '\u{2578}' => [H, N, N, N], // ╸
        '\u{2579}' => [N, N, H, N], // ╹
        '\u{257A}' => [N, H, N, N], // ╺
        '\u{257B}' => [N, N, N, H], // ╻
        '\u{257C}' => [L, H, N, N], // ╼
        '\u{257D}' => [N, N, L, H], // ╽
        '\u{257E}' => [H, L, N, N], // ╾
        '\u{257F}' => [N, N, H, L], // ╿
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    /// Every box-drawing char is vector-drawn except the diagonals.
    ///
    /// A font glyph sits on the font's own bounding box and stroke
    /// weight, so whatever is left to the fallback disagrees with the
    /// vector strokes it meets in the same table: `┼` was the visible
    /// one — a table's crossings rendered heavier than its borders.
    #[test]
    fn the_whole_box_drawing_block_is_vector() {
        let font_fallback = ['╱', '╲', '╳'];
        for cp in 0x2500..=0x257F {
            let c = char::from_u32(cp).unwrap();
            assert_eq!(
                super::is_vector(c),
                !font_fallback.contains(&c),
                "U+{cp:04X} {c}"
            );
        }
        // Arcs are vector too (drawn as stroked quads).
        for c in ['╭', '╮', '╯', '╰'] {
            assert!(super::is_vector(c), "{c}");
        }
    }

    #[test]
    fn vector_coverage_matches_intent() {
        // TUI staples are vector-drawn; diagonals stay font glyphs.
        for (c, want) in [
            ('─', true),
            ('│', true),
            ('┌', true),
            ('╬', true),
            ('╭', true),
            ('█', true),
            ('▄', true),
            ('░', true),
            ('▛', true),
            ('╱', false),
            ('╳', false),
        ] {
            assert_eq!(super::is_vector(c), want, "{c}");
        }
        // Text must never go down the vector path.
        for c in ['a', '中', ' ', '\0', '●'] {
            assert!(!super::is_vector(c), "{c}");
        }
    }
}
