//! Box Drawing and Block Elements, drawn from the cell box instead of the font.
//!
//! **Why.** A font glyph fills the font's *em box*, not our *cell*. The cell is
//! `max(size × line_height, ascent + descent)`, so with any leading at all a
//! `▀▄█` logo tiles with a seam between every row, and on Retina the seam is a
//! whole device pixel. Worse, the family may simply not have the glyph —
//! Monaco is missing 11 of the 16 block shapes Claude Code's and `opencode`'s
//! logos use — and every missing one falls back to a face whose metrics do not
//! match the cell, which fragments the shape. iTerm2, Kitty, Ghostty, Alacritty,
//! VTE and xterm.js all draw these ranges themselves for exactly this reason.
//! Alacritty's commit message is the one-line justification: the font "tends to
//! overlap or not align, so providing built-in font is the lesser evil."
//!
//! **What.** Pure geometry, no gpui. [`sprite_for`] turns a codepoint in
//! U+2500–U+259F into rectangles in cell-local pixels (or, for the four rounded
//! corners `╭╮╯╰`, a quarter-circle description), and `paint.rs` turns those
//! into quads. Block Elements are exact eighths, halves and quadrants of the
//! cell; Box Drawing is four arms out from the centre in light, heavy or double
//! weight, plus the twelve dashed forms.
//!
//! **What is left to the font, on purpose.** The diagonals `╱╲╳`
//! (U+2571–U+2573): they are not rectangles. [`is_sprite`] excludes them.
//!
//! **Rounding is the painter's job.** Everything here is fractional. Two
//! neighbouring cells derive their shared edge from the same grid line, so
//! when `paint.rs` rounds both in window space they land on the same integer:
//! no seam, no overlap. `ceil()` would be wrong there — see `paint_sprite_run`.

/// A rectangle in cell-local pixels, `x`/`y` from the cell's top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge.
    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Which corner of an [`Arc`]'s bounding box is the rounded one. The stroke
/// runs along the two edges that meet at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    /// `╭` (U+256D): stroke along the top and left edges.
    TopLeft,
    /// `╮` (U+256E): top and right.
    TopRight,
    /// `╯` (U+256F): bottom and right.
    BottomRight,
    /// `╰` (U+2570): bottom and left.
    BottomLeft,
}

/// A rounded corner: one quad with no fill, a border on the two edges that meet
/// at [`Arc::corner`], and a radius on that corner. GPUI draws the border along
/// the rounded outline, which is a true quarter circle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arc {
    /// The quad, in cell-local pixels. Its two stroked edges sit on the cell's
    /// centre lines, so the stroke's centreline passes through the centre of
    /// the cell edges it exits through, exactly like the straight arms.
    pub bounds: Rect,
    /// The rounded corner.
    pub corner: Corner,
    /// Stroke thickness (light weight).
    pub thickness: f32,
    /// Outer radius of the rounded corner — the stroke's centreline radius
    /// plus half the thickness.
    pub radius: f32,
}

/// What one sprite codepoint paints.
#[derive(Debug, Clone, PartialEq)]
pub enum Sprite {
    /// Solid rectangles, painted in the run's foreground at `alpha` (1.0 for
    /// everything but the three shades `░▒▓`).
    Rects {
        /// The rectangles.
        rects: Vec<Rect>,
        /// Fill opacity.
        alpha: f32,
    },
    /// One of the four rounded corners.
    Arc(Arc),
}

/// `true` for a codepoint this module draws. The painter never shapes these
/// through the font, and `runs.rs` keeps them in runs of their own.
#[must_use]
pub fn is_sprite(ch: char) -> bool {
    let code = ch as u32;
    // Box Drawing U+2500–U+257F minus the three diagonals, and every Block
    // Element U+2580–U+259F.
    (0x2500..=0x259F).contains(&code) && !(0x2571..=0x2573).contains(&code)
}

/// The geometry for `ch` in a `w × h` cell, or `None` when it is not a sprite.
#[must_use]
pub fn sprite_for(ch: char, w: f32, h: f32) -> Option<Sprite> {
    if !is_sprite(ch) {
        return None;
    }
    let cell = Cell::new(w, h);
    // From here on `cell.w`/`cell.h`: a degenerate size has been sanitised.
    let (w, h) = (cell.w, cell.h);
    let code = ch as u32;
    if let Some(alpha) = shade_alpha(ch) {
        return Some(Sprite::Rects {
            rects: vec![Rect::new(0.0, 0.0, w, h)],
            alpha,
        });
    }
    if (0x2580..=0x259F).contains(&code) {
        return Some(Sprite::Rects {
            rects: block_rects(ch, w, h),
            alpha: 1.0,
        });
    }
    if let Some(corner) = arc_corner(ch) {
        return Some(Sprite::Arc(cell.arc(corner)));
    }
    if let Some((count, weight, horizontal)) = dashed(ch) {
        return Some(Sprite::Rects {
            rects: cell.dashes(count, weight, horizontal),
            alpha: 1.0,
        });
    }
    if let Some(strokes) = double_strokes(ch) {
        return Some(Sprite::Rects {
            rects: cell.double(&strokes),
            alpha: 1.0,
        });
    }
    let arms = arms(ch)?;
    Some(Sprite::Rects {
        rects: cell.arms(arms),
        alpha: 1.0,
    })
}

/// Rectangles only — the form most callers and tests want. `None` for a
/// non-sprite and for the four arcs.
#[must_use]
pub fn rects_for(ch: char, w: f32, h: f32) -> Option<Vec<Rect>> {
    match sprite_for(ch, w, h)? {
        Sprite::Rects { rects, .. } => Some(rects),
        Sprite::Arc(_) => None,
    }
}

/// Opacity for the three shade blocks; `None` for anything else.
#[must_use]
pub fn shade_alpha(ch: char) -> Option<f32> {
    match ch {
        '░' => Some(0.25),
        '▒' => Some(0.5),
        '▓' => Some(0.75),
        _ => None,
    }
}

// ---------------------------------------------------------------- blocks --

/// Block Elements U+2580–U+259F as fractions of the cell.
fn block_rects(ch: char, w: f32, h: f32) -> Vec<Rect> {
    let eighth_h = h / 8.0;
    let eighth_w = w / 8.0;
    let lower = |n: f32| vec![Rect::new(0.0, h - eighth_h * n, w, eighth_h * n)];
    let left = |n: f32| vec![Rect::new(0.0, 0.0, eighth_w * n, h)];
    let hw = w / 2.0;
    let hh = h / 2.0;
    // Quadrants, top-left origin.
    let ul = Rect::new(0.0, 0.0, hw, hh);
    let ur = Rect::new(hw, 0.0, w - hw, hh);
    let ll = Rect::new(0.0, hh, hw, h - hh);
    let lr = Rect::new(hw, hh, w - hw, h - hh);
    match ch {
        '▀' => vec![Rect::new(0.0, 0.0, w, hh)],
        '▁' => lower(1.0),
        '▂' => lower(2.0),
        '▃' => lower(3.0),
        '▄' => lower(4.0),
        '▅' => lower(5.0),
        '▆' => lower(6.0),
        '▇' => lower(7.0),
        '█' => vec![Rect::new(0.0, 0.0, w, h)],
        '▉' => left(7.0),
        '▊' => left(6.0),
        '▋' => left(5.0),
        '▌' => left(4.0),
        '▍' => left(3.0),
        '▎' => left(2.0),
        '▏' => left(1.0),
        '▐' => vec![Rect::new(hw, 0.0, w - hw, h)],
        '▔' => vec![Rect::new(0.0, 0.0, w, eighth_h)],
        '▕' => vec![Rect::new(w - eighth_w, 0.0, eighth_w, h)],
        '▖' => vec![ll],
        '▗' => vec![lr],
        '▘' => vec![ul],
        '▙' => vec![ul, ll, lr],
        '▚' => vec![ul, lr],
        '▛' => vec![ul, ur, ll],
        '▜' => vec![ul, ur, lr],
        '▝' => vec![ur],
        '▞' => vec![ur, ll],
        '▟' => vec![ur, ll, lr],
        // The shades are handled before we get here.
        _ => Vec::new(),
    }
}

// ------------------------------------------------------------- box lines --

/// Stroke weight of one arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    /// No arm on this side.
    None,
    /// Light (single, thin).
    Light,
    /// Heavy (single, thick).
    Heavy,
}

/// The four arms of a single-weight box character: up, down, left, right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arms {
    /// Towards the top edge.
    pub up: Weight,
    /// Towards the bottom edge.
    pub down: Weight,
    /// Towards the left edge.
    pub left: Weight,
    /// Towards the right edge.
    pub right: Weight,
}

const N: Weight = Weight::None;
const L: Weight = Weight::Light;
const H: Weight = Weight::Heavy;

const fn a(up: Weight, down: Weight, left: Weight, right: Weight) -> Arms {
    Arms {
        up,
        down,
        left,
        right,
    }
}

/// The arm table for the light/heavy set (U+2500–U+254B, U+2574–U+257F),
/// straight from the Unicode names ("DOWN LIGHT AND RIGHT HEAVY" and so on).
/// Order in every entry: up, down, left, right.
#[must_use]
pub fn arms(ch: char) -> Option<Arms> {
    Some(match ch {
        '─' => a(N, N, L, L),
        '━' => a(N, N, H, H),
        '│' => a(L, L, N, N),
        '┃' => a(H, H, N, N),
        '┌' => a(N, L, N, L),
        '┍' => a(N, L, N, H),
        '┎' => a(N, H, N, L),
        '┏' => a(N, H, N, H),
        '┐' => a(N, L, L, N),
        '┑' => a(N, L, H, N),
        '┒' => a(N, H, L, N),
        '┓' => a(N, H, H, N),
        '└' => a(L, N, N, L),
        '┕' => a(L, N, N, H),
        '┖' => a(H, N, N, L),
        '┗' => a(H, N, N, H),
        '┘' => a(L, N, L, N),
        '┙' => a(L, N, H, N),
        '┚' => a(H, N, L, N),
        '┛' => a(H, N, H, N),
        '├' => a(L, L, N, L),
        '┝' => a(L, L, N, H),
        '┞' => a(H, L, N, L),
        '┟' => a(L, H, N, L),
        '┠' => a(H, H, N, L),
        '┡' => a(H, L, N, H),
        '┢' => a(L, H, N, H),
        '┣' => a(H, H, N, H),
        '┤' => a(L, L, L, N),
        '┥' => a(L, L, H, N),
        '┦' => a(H, L, L, N),
        '┧' => a(L, H, L, N),
        '┨' => a(H, H, L, N),
        '┩' => a(H, L, H, N),
        '┪' => a(L, H, H, N),
        '┫' => a(H, H, H, N),
        '┬' => a(N, L, L, L),
        '┭' => a(N, L, H, L),
        '┮' => a(N, L, L, H),
        '┯' => a(N, L, H, H),
        '┰' => a(N, H, L, L),
        '┱' => a(N, H, H, L),
        '┲' => a(N, H, L, H),
        '┳' => a(N, H, H, H),
        '┴' => a(L, N, L, L),
        '┵' => a(L, N, H, L),
        '┶' => a(L, N, L, H),
        '┷' => a(L, N, H, H),
        '┸' => a(H, N, L, L),
        '┹' => a(H, N, H, L),
        '┺' => a(H, N, L, H),
        '┻' => a(H, N, H, H),
        '┼' => a(L, L, L, L),
        '┽' => a(L, L, H, L),
        '┾' => a(L, L, L, H),
        '┿' => a(L, L, H, H),
        '╀' => a(H, L, L, L),
        '╁' => a(L, H, L, L),
        '╂' => a(H, H, L, L),
        '╃' => a(H, L, H, L),
        '╄' => a(H, L, L, H),
        '╅' => a(L, H, H, L),
        '╆' => a(L, H, L, H),
        '╇' => a(H, L, H, H),
        '╈' => a(L, H, H, H),
        '╉' => a(H, H, H, L),
        '╊' => a(H, H, L, H),
        '╋' => a(H, H, H, H),
        '╴' => a(N, N, L, N),
        '╵' => a(L, N, N, N),
        '╶' => a(N, N, N, L),
        '╷' => a(N, L, N, N),
        '╸' => a(N, N, H, N),
        '╹' => a(H, N, N, N),
        '╺' => a(N, N, N, H),
        '╻' => a(N, H, N, N),
        '╼' => a(N, N, L, H),
        '╽' => a(L, H, N, N),
        '╾' => a(N, N, H, L),
        '╿' => a(H, L, N, N),
        _ => return None,
    })
}

/// `(dash count, weight, horizontal)` for the twelve dashed forms.
fn dashed(ch: char) -> Option<(u32, Weight, bool)> {
    Some(match ch {
        '┄' => (3, L, true),
        '┅' => (3, H, true),
        '┆' => (3, L, false),
        '┇' => (3, H, false),
        '┈' => (4, L, true),
        '┉' => (4, H, true),
        '┊' => (4, L, false),
        '┋' => (4, H, false),
        '╌' => (2, L, true),
        '╍' => (2, H, true),
        '╎' => (2, L, false),
        '╏' => (2, H, false),
        _ => return None,
    })
}

fn arc_corner(ch: char) -> Option<Corner> {
    Some(match ch {
        '╭' => Corner::TopLeft,
        '╮' => Corner::TopRight,
        '╯' => Corner::BottomRight,
        '╰' => Corner::BottomLeft,
        _ => return None,
    })
}

/// A symbolic position along one axis, resolved against the cell by
/// [`Cell::x_of`] / [`Cell::y_of`]. `Minus`/`Plus` are the two strokes of a
/// double line, `d` either side of the centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum P {
    /// The cell edge at 0.
    Edge0,
    /// Centre minus the double-line offset.
    Minus,
    /// The centre line.
    Centre,
    /// Centre plus the double-line offset.
    Plus,
    /// The far cell edge.
    Edge1,
}

/// One light stroke of a double-line character: horizontal at the `y`
/// position from `x0` to `x1`, or vertical at `x` from `y0` to `y1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stroke {
    H(P, P, P),
    V(P, P, P),
}

use P::{Centre, Edge0, Edge1, Minus, Plus};
use Stroke::{H as SH, V as SV};

/// The double-line set (U+2550–U+256C), hand-listed so every join is right:
/// an outer corner meets the outer stroke, an inner corner the inner one, and
/// a single stem stops at the near double stroke instead of poking through.
fn double_strokes(ch: char) -> Option<Vec<Stroke>> {
    Some(match ch {
        '═' => vec![SH(Minus, Edge0, Edge1), SH(Plus, Edge0, Edge1)],
        '║' => vec![SV(Minus, Edge0, Edge1), SV(Plus, Edge0, Edge1)],
        '╒' => vec![SV(Centre, Minus, Edge1), SH(Minus, Centre, Edge1), SH(Plus, Centre, Edge1)],
        '╓' => vec![SV(Minus, Centre, Edge1), SV(Plus, Centre, Edge1), SH(Centre, Minus, Edge1)],
        '╔' => vec![
            SV(Minus, Minus, Edge1),
            SV(Plus, Plus, Edge1),
            SH(Minus, Minus, Edge1),
            SH(Plus, Plus, Edge1),
        ],
        '╕' => vec![SV(Centre, Minus, Edge1), SH(Minus, Edge0, Centre), SH(Plus, Edge0, Centre)],
        '╖' => vec![SV(Minus, Centre, Edge1), SV(Plus, Centre, Edge1), SH(Centre, Edge0, Plus)],
        '╗' => vec![
            SV(Plus, Minus, Edge1),
            SV(Minus, Plus, Edge1),
            SH(Minus, Edge0, Plus),
            SH(Plus, Edge0, Minus),
        ],
        '╘' => vec![SV(Centre, Edge0, Plus), SH(Minus, Centre, Edge1), SH(Plus, Centre, Edge1)],
        '╙' => vec![SV(Minus, Edge0, Centre), SV(Plus, Edge0, Centre), SH(Centre, Minus, Edge1)],
        '╚' => vec![
            SV(Minus, Edge0, Plus),
            SV(Plus, Edge0, Minus),
            SH(Plus, Minus, Edge1),
            SH(Minus, Plus, Edge1),
        ],
        '╛' => vec![SV(Centre, Edge0, Plus), SH(Minus, Edge0, Centre), SH(Plus, Edge0, Centre)],
        '╜' => vec![SV(Minus, Edge0, Centre), SV(Plus, Edge0, Centre), SH(Centre, Edge0, Plus)],
        '╝' => vec![
            SV(Plus, Edge0, Plus),
            SV(Minus, Edge0, Minus),
            SH(Plus, Edge0, Plus),
            SH(Minus, Edge0, Minus),
        ],
        '╞' => vec![SV(Centre, Edge0, Edge1), SH(Minus, Centre, Edge1), SH(Plus, Centre, Edge1)],
        '╟' => vec![SV(Minus, Edge0, Edge1), SV(Plus, Edge0, Edge1), SH(Centre, Plus, Edge1)],
        '╠' => vec![
            SV(Minus, Edge0, Edge1),
            SV(Plus, Edge0, Edge1),
            SH(Minus, Plus, Edge1),
            SH(Plus, Plus, Edge1),
        ],
        '╡' => vec![SV(Centre, Edge0, Edge1), SH(Minus, Edge0, Centre), SH(Plus, Edge0, Centre)],
        '╢' => vec![SV(Minus, Edge0, Edge1), SV(Plus, Edge0, Edge1), SH(Centre, Edge0, Minus)],
        '╣' => vec![
            SV(Minus, Edge0, Edge1),
            SV(Plus, Edge0, Edge1),
            SH(Minus, Edge0, Minus),
            SH(Plus, Edge0, Minus),
        ],
        '╤' => vec![SH(Minus, Edge0, Edge1), SH(Plus, Edge0, Edge1), SV(Centre, Plus, Edge1)],
        '╥' => vec![SH(Centre, Edge0, Edge1), SV(Minus, Centre, Edge1), SV(Plus, Centre, Edge1)],
        '╦' => vec![
            SH(Minus, Edge0, Edge1),
            SH(Plus, Edge0, Edge1),
            SV(Minus, Plus, Edge1),
            SV(Plus, Plus, Edge1),
        ],
        '╧' => vec![SH(Minus, Edge0, Edge1), SH(Plus, Edge0, Edge1), SV(Centre, Edge0, Minus)],
        '╨' => vec![SH(Centre, Edge0, Edge1), SV(Minus, Edge0, Centre), SV(Plus, Edge0, Centre)],
        '╩' => vec![
            SH(Minus, Edge0, Edge1),
            SH(Plus, Edge0, Edge1),
            SV(Minus, Edge0, Minus),
            SV(Plus, Edge0, Minus),
        ],
        '╪' => vec![SH(Minus, Edge0, Edge1), SH(Plus, Edge0, Edge1), SV(Centre, Edge0, Edge1)],
        '╫' => vec![SH(Centre, Edge0, Edge1), SV(Minus, Edge0, Edge1), SV(Plus, Edge0, Edge1)],
        '╬' => vec![
            SH(Minus, Edge0, Edge1),
            SH(Plus, Edge0, Edge1),
            SV(Minus, Edge0, Edge1),
            SV(Plus, Edge0, Edge1),
        ],
        _ => return None,
    })
}

/// The cell's derived stroke metrics.
#[derive(Debug, Clone, Copy)]
struct Cell {
    w: f32,
    h: f32,
    /// Centre.
    cx: f32,
    cy: f32,
    /// Light stroke thickness.
    light: f32,
    /// Heavy stroke thickness.
    heavy: f32,
    /// Offset of each double-line stroke from the centre.
    d: f32,
}

impl Cell {
    fn new(w: f32, h: f32) -> Self {
        let w = if w.is_finite() && w > 0.0 { w } else { 1.0 };
        let h = if h.is_finite() && h > 0.0 { h } else { 1.0 };
        // ~1 px up to a 12 px cell, 2 px to 20 px, and so on: the stroke grows
        // with the font instead of staying a hairline at 24 pt.
        let light = (w / 8.0).round().max(1.0);
        let heavy = light * 2.0 + 1.0;
        let d = (w / 6.0).round().max(light);
        Self {
            w,
            h,
            cx: w / 2.0,
            cy: h / 2.0,
            light,
            heavy,
            d,
        }
    }

    fn thickness(&self, weight: Weight) -> f32 {
        match weight {
            Weight::None => 0.0,
            Weight::Light => self.light,
            Weight::Heavy => self.heavy,
        }
    }

    /// The four arms, each running from its cell edge to the centre and
    /// overshooting by half the thickest perpendicular arm so the joint is a
    /// solid square whatever the weights.
    fn arms(&self, arms: Arms) -> Vec<Rect> {
        let vertical = self
            .thickness(arms.up)
            .max(self.thickness(arms.down));
        let horizontal = self
            .thickness(arms.left)
            .max(self.thickness(arms.right));
        let mut out = Vec::with_capacity(4);
        let reach_h = |own: f32| vertical.max(own) / 2.0;
        let reach_v = |own: f32| horizontal.max(own) / 2.0;

        let k = self.thickness(arms.left);
        if k > 0.0 {
            let end = self.cx + reach_h(k);
            out.push(Rect::new(0.0, self.cy - k / 2.0, end, k));
        }
        let k = self.thickness(arms.right);
        if k > 0.0 {
            let start = self.cx - reach_h(k);
            out.push(Rect::new(start, self.cy - k / 2.0, self.w - start, k));
        }
        let k = self.thickness(arms.up);
        if k > 0.0 {
            let end = self.cy + reach_v(k);
            out.push(Rect::new(self.cx - k / 2.0, 0.0, k, end));
        }
        let k = self.thickness(arms.down);
        if k > 0.0 {
            let start = self.cy - reach_v(k);
            out.push(Rect::new(self.cx - k / 2.0, start, k, self.h - start));
        }
        out
    }

    /// `count` dashes along one axis, each `period - gap` long.
    fn dashes(&self, count: u32, weight: Weight, horizontal: bool) -> Vec<Rect> {
        let k = self.thickness(weight);
        let length = if horizontal { self.w } else { self.h };
        let period = length / count as f32;
        let gap = (period / 3.0).max(1.0).min(period / 2.0);
        (0..count)
            .map(|i| {
                let start = i as f32 * period + gap / 2.0;
                let dash = period - gap;
                if horizontal {
                    Rect::new(start, self.cy - k / 2.0, dash, k)
                } else {
                    Rect::new(self.cx - k / 2.0, start, k, dash)
                }
            })
            .collect()
    }

    fn x_of(&self, p: P) -> f32 {
        match p {
            Edge0 => 0.0,
            Minus => self.cx - self.d,
            Centre => self.cx,
            Plus => self.cx + self.d,
            Edge1 => self.w,
        }
    }

    fn y_of(&self, p: P) -> f32 {
        match p {
            Edge0 => 0.0,
            Minus => self.cy - self.d,
            Centre => self.cy,
            Plus => self.cy + self.d,
            Edge1 => self.h,
        }
    }

    /// Light strokes; an end that stops at an interior line is extended by
    /// half a stroke so the joint square is covered.
    fn double(&self, strokes: &[Stroke]) -> Vec<Rect> {
        let t = self.light;
        let extend = |p: P, at: f32, towards_start: bool| -> f32 {
            match p {
                Edge0 | Edge1 => at,
                _ if towards_start => at - t / 2.0,
                _ => at + t / 2.0,
            }
        };
        strokes
            .iter()
            .map(|stroke| match *stroke {
                SH(y, x0, x1) => {
                    let start = extend(x0, self.x_of(x0), true);
                    let end = extend(x1, self.x_of(x1), false);
                    Rect::new(start, self.y_of(y) - t / 2.0, end - start, t)
                }
                SV(x, y0, y1) => {
                    let start = extend(y0, self.y_of(y0), true);
                    let end = extend(y1, self.y_of(y1), false);
                    Rect::new(self.x_of(x) - t / 2.0, start, t, end - start)
                }
            })
            .collect()
    }

    /// A quarter circle of radius `min(cx, cy)` joining the two arms, which
    /// then continue straight to their cell edges. Rounder than iTerm's
    /// bezier, which spans the whole cell, but a circle is what `quad()` can
    /// draw and the difference is invisible at cell size.
    fn arc(&self, corner: Corner) -> Arc {
        let t = self.light;
        let half = t / 2.0;
        let radius = self.cx.min(self.cy) + half;
        let bounds = match corner {
            Corner::TopLeft => Rect::new(
                self.cx - half,
                self.cy - half,
                self.w - (self.cx - half),
                self.h - (self.cy - half),
            ),
            Corner::TopRight => Rect::new(0.0, self.cy - half, self.cx + half, self.h - (self.cy - half)),
            Corner::BottomRight => Rect::new(0.0, 0.0, self.cx + half, self.cy + half),
            Corner::BottomLeft => Rect::new(self.cx - half, 0.0, self.w - (self.cx - half), self.cy + half),
        };
        Arc {
            bounds,
            corner,
            thickness: t,
            radius,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 8.0;
    const HT: f32 = 17.0;

    fn rects(ch: char) -> Vec<Rect> {
        rects_for(ch, W, HT).unwrap_or_else(|| panic!("{ch:?} is not a rect sprite"))
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Total area of a set of non-overlapping rects.
    fn area(rects: &[Rect]) -> f32 {
        rects.iter().map(|r| r.w * r.h).sum()
    }

    #[test]
    fn the_sprite_range_is_box_drawing_and_block_elements_minus_the_diagonals() {
        assert!(is_sprite('─'));
        assert!(is_sprite('╿'), "last box drawing codepoint");
        assert!(is_sprite('▀'));
        assert!(is_sprite('▟'), "last block element");
        assert!(is_sprite('╭'));
        assert!(!is_sprite('╱'), "diagonals are left to the font");
        assert!(!is_sprite('╲'));
        assert!(!is_sprite('╳'));
        assert!(!is_sprite('■'), "geometric shapes are not ours");
        assert!(!is_sprite('a'));
        assert!(!is_sprite(' '));
        assert!(!is_sprite('⠿'), "braille is still open (handover §4)");
    }

    #[test]
    fn every_sprite_codepoint_has_geometry() {
        for code in 0x2500..=0x259Fu32 {
            let ch = char::from_u32(code).unwrap();
            if !is_sprite(ch) {
                continue;
            }
            let sprite = sprite_for(ch, W, HT).unwrap_or_else(|| panic!("U+{code:04X} has no sprite"));
            if let Sprite::Rects { rects, .. } = &sprite {
                assert!(!rects.is_empty(), "U+{code:04X} paints nothing");
                for r in rects {
                    assert!(r.w > 0.0 && r.h > 0.0, "U+{code:04X} has a degenerate rect {r:?}");
                    assert!(r.x >= -0.01 && r.y >= -0.01, "U+{code:04X} starts outside the cell: {r:?}");
                    assert!(r.right() <= W + 0.01 && r.bottom() <= HT + 0.01, "U+{code:04X} leaves the cell: {r:?}");
                }
            }
        }
    }

    #[test]
    fn a_non_sprite_has_no_geometry() {
        assert_eq!(sprite_for('a', W, HT), None);
        assert_eq!(rects_for('╱', W, HT), None);
    }

    #[test]
    fn the_full_block_is_the_whole_cell() {
        assert_eq!(rects('█'), vec![Rect::new(0.0, 0.0, W, HT)]);
    }

    #[test]
    fn upper_and_lower_halves_tile_the_cell_with_no_seam() {
        let upper = rects('▀');
        let lower = rects('▄');
        assert_eq!(upper.len(), 1);
        assert_eq!(lower.len(), 1);
        assert!(approx(upper[0].y, 0.0));
        assert!(approx(upper[0].bottom(), lower[0].y), "the shared edge is the same grid line");
        assert!(approx(lower[0].bottom(), HT));
        assert!(approx(area(&upper) + area(&lower), W * HT));
    }

    #[test]
    fn left_and_right_halves_tile_the_cell() {
        let left = rects('▌');
        let right = rects('▐');
        assert!(approx(left[0].right(), right[0].x));
        assert!(approx(area(&left) + area(&right), W * HT));
    }

    #[test]
    fn lower_eighths_grow_from_the_bottom_in_exact_eighths() {
        let chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        for (i, ch) in chars.iter().enumerate() {
            let r = rects(*ch);
            assert_eq!(r.len(), 1);
            let n = (i + 1) as f32;
            assert!(approx(r[0].h, HT * n / 8.0), "{ch}: {:?}", r[0]);
            assert!(approx(r[0].bottom(), HT), "{ch} is anchored to the bottom");
            assert!(approx(r[0].w, W));
        }
    }

    #[test]
    fn left_eighths_grow_from_the_left() {
        let chars = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
        for (i, ch) in chars.iter().enumerate() {
            let r = rects(*ch);
            let n = (i + 1) as f32;
            assert!(approx(r[0].w, W * n / 8.0), "{ch}: {:?}", r[0]);
            assert!(approx(r[0].x, 0.0));
        }
    }

    #[test]
    fn the_quadrant_set_covers_the_cell_exactly_once_per_quadrant() {
        // The four single quadrants together are the whole cell…
        let singles: Vec<Rect> = ['▘', '▝', '▖', '▗'].iter().flat_map(|c| rects(*c)).collect();
        assert!(approx(area(&singles), W * HT));
        // …and each composite is the sum of its quadrants.
        assert!(approx(area(&rects('▙')), W * HT * 0.75));
        assert!(approx(area(&rects('▚')), W * HT * 0.5));
        assert!(approx(area(&rects('▞')), W * HT * 0.5));
        assert!(approx(area(&rects('▛')), W * HT * 0.75));
        assert!(approx(area(&rects('▜')), W * HT * 0.75));
        assert!(approx(area(&rects('▟')), W * HT * 0.75));
        // ▚ and ▞ are complementary.
        assert!(approx(area(&rects('▚')) + area(&rects('▞')), W * HT));
    }

    #[test]
    fn the_shades_are_a_full_cell_at_a_quarter_half_and_three_quarters() {
        for (ch, alpha) in [('░', 0.25), ('▒', 0.5), ('▓', 0.75)] {
            match sprite_for(ch, W, HT).unwrap() {
                Sprite::Rects { rects, alpha: got } => {
                    assert_eq!(rects, vec![Rect::new(0.0, 0.0, W, HT)]);
                    assert!(approx(got, alpha));
                }
                other => panic!("{ch}: {other:?}"),
            }
        }
        assert_eq!(shade_alpha('█'), None);
    }

    #[test]
    fn a_light_horizontal_spans_the_cell_on_the_centre_line() {
        let r = rects('─');
        assert_eq!(r.len(), 2, "left arm and right arm");
        let min_x = r.iter().map(|r| r.x).fold(f32::MAX, f32::min);
        let max_x = r.iter().map(Rect::right).fold(f32::MIN, f32::max);
        assert!(approx(min_x, 0.0));
        assert!(approx(max_x, W));
        for arm in &r {
            assert!(approx(arm.y + arm.h / 2.0, HT / 2.0), "{arm:?} is not centred");
            assert!(approx(arm.h, 1.0), "an 8 px cell gets a 1 px light stroke");
        }
        // The two arms overlap at the centre rather than leaving a gap.
        assert!(r[0].right() >= r[1].x);
    }

    #[test]
    fn a_light_vertical_spans_the_cell_height() {
        let r = rects('│');
        let min_y = r.iter().map(|r| r.y).fold(f32::MAX, f32::min);
        let max_y = r.iter().map(Rect::bottom).fold(f32::MIN, f32::max);
        assert!(approx(min_y, 0.0));
        assert!(approx(max_y, HT));
        for arm in &r {
            assert!(approx(arm.x + arm.w / 2.0, W / 2.0));
        }
    }

    #[test]
    fn neighbouring_lines_meet_on_the_same_grid_line() {
        // A ─ in this cell ends at x = W; the next cell's ─ starts at 0 in its
        // own frame, i.e. at the same window x. Likewise │ across rows.
        let h = rects('─');
        assert!(h.iter().any(|r| approx(r.right(), W)));
        assert!(h.iter().any(|r| approx(r.x, 0.0)));
        let v = rects('│');
        assert!(v.iter().any(|r| approx(r.bottom(), HT)));
        assert!(v.iter().any(|r| approx(r.y, 0.0)));
    }

    #[test]
    fn a_corner_has_two_arms_that_share_the_joint_square() {
        let r = rects('┌');
        assert_eq!(r.len(), 2);
        let right = r.iter().find(|r| approx(r.right(), W)).expect("right arm");
        let down = r.iter().find(|r| approx(r.bottom(), HT)).expect("down arm");
        // The right arm starts at the vertical stroke's left edge, the down arm
        // at the horizontal stroke's top edge: the joint is fully covered.
        assert!(approx(right.x, down.x), "{right:?} vs {down:?}");
        assert!(approx(down.y, right.y), "{right:?} vs {down:?}");
        // No arm reaches the opposite edges.
        assert!(right.x > 0.0);
        assert!(down.y > 0.0);
    }

    #[test]
    fn heavy_is_thicker_than_light_and_mixed_joints_are_solid() {
        let light = rects('│');
        let heavy = rects('┃');
        assert!(heavy[0].w > light[0].w);
        assert!(approx(heavy[0].w, 3.0), "8 px cell: 1 px light, 3 px heavy");

        // ┍: down light, right heavy. The heavy arm must reach back past the
        // light stem so there is no notch at the corner.
        let r = rects('┍');
        let right = r.iter().find(|r| approx(r.right(), W)).unwrap();
        let down = r.iter().find(|r| approx(r.bottom(), HT)).unwrap();
        assert!(approx(right.h, 3.0));
        assert!(approx(down.w, 1.0));
        assert!(right.x <= down.x + 1e-4);
        assert!(down.y <= right.y + 1e-4);
    }

    #[test]
    fn the_half_arms_stop_at_the_centre() {
        let r = rects('╶');
        assert_eq!(r.len(), 1);
        assert!(approx(r[0].right(), W));
        assert!((r[0].x - W / 2.0).abs() <= 0.5, "{:?}", r[0]);
        let r = rects('╵');
        assert!(approx(r[0].y, 0.0));
        assert!((r[0].bottom() - HT / 2.0).abs() <= 0.5, "{:?}", r[0]);
    }

    #[test]
    fn dashes_come_in_two_three_and_four_with_gaps_between() {
        for (ch, n) in [('╌', 2), ('┄', 3), ('┈', 4)] {
            let r = rects(ch);
            assert_eq!(r.len(), n, "{ch}");
            for pair in r.windows(2) {
                assert!(pair[0].right() < pair[1].x, "{ch}: dashes touch {pair:?}");
            }
            assert!(r[0].x > 0.0 && r[n - 1].right() < W, "{ch}: a gap at each end too");
        }
        for (ch, n) in [('╎', 2), ('┆', 3), ('┊', 4)] {
            let r = rects(ch);
            assert_eq!(r.len(), n, "{ch}");
            for pair in r.windows(2) {
                assert!(pair[0].bottom() < pair[1].y, "{ch}");
            }
        }
        assert!(rects('┅')[0].h > rects('┄')[0].h, "heavy dashes are thicker");
    }

    #[test]
    fn a_double_line_is_two_parallel_light_strokes() {
        let r = rects('═');
        assert_eq!(r.len(), 2);
        assert!(approx(r[0].x, 0.0) && approx(r[0].right(), W));
        assert!(approx(r[1].x, 0.0) && approx(r[1].right(), W));
        assert!(r[0].bottom() < r[1].y, "a gap between the strokes");
        assert!(approx(r[0].h, 1.0) && approx(r[1].h, 1.0));
        // Symmetric about the centre line.
        let mid0 = r[0].y + r[0].h / 2.0;
        let mid1 = r[1].y + r[1].h / 2.0;
        assert!(approx((mid0 + mid1) / 2.0, HT / 2.0));
    }

    #[test]
    fn a_double_corner_joins_outer_to_outer_and_inner_to_inner() {
        let r = rects('╔');
        assert_eq!(r.len(), 4);
        let verticals: Vec<&Rect> = r.iter().filter(|r| approx(r.bottom(), HT)).collect();
        let horizontals: Vec<&Rect> = r.iter().filter(|r| approx(r.right(), W)).collect();
        assert_eq!(verticals.len(), 2);
        assert_eq!(horizontals.len(), 2);
        let outer_v = verticals.iter().min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
        let inner_v = verticals.iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
        let outer_h = horizontals.iter().min_by(|a, b| a.y.total_cmp(&b.y)).unwrap();
        let inner_h = horizontals.iter().max_by(|a, b| a.y.total_cmp(&b.y)).unwrap();
        // Outer: the vertical starts at the horizontal's top edge and the
        // horizontal at the vertical's left edge.
        assert!(approx(outer_v.y, outer_h.y), "{outer_v:?} / {outer_h:?}");
        assert!(approx(outer_h.x, outer_v.x));
        // Inner likewise, and it starts strictly inside the outer corner.
        assert!(approx(inner_v.y, inner_h.y));
        assert!(approx(inner_h.x, inner_v.x));
        assert!(inner_v.x > outer_v.x && inner_h.y > outer_h.y);
    }

    #[test]
    fn a_single_stem_on_a_double_bar_stops_at_the_near_stroke() {
        // ╤: the stem hangs from the LOWER of the two horizontals.
        let r = rects('╤');
        let stem = r.iter().find(|r| approx(r.bottom(), HT)).unwrap();
        let lower_bar = r
            .iter()
            .filter(|r| approx(r.right(), W))
            .max_by(|a, b| a.y.total_cmp(&b.y))
            .unwrap();
        assert!(approx(stem.y, lower_bar.y), "{stem:?} should start at {lower_bar:?}");
    }

    #[test]
    fn the_rounded_corners_describe_a_quarter_circle_on_the_right_corner() {
        for (ch, corner) in [
            ('╭', Corner::TopLeft),
            ('╮', Corner::TopRight),
            ('╯', Corner::BottomRight),
            ('╰', Corner::BottomLeft),
        ] {
            let Some(Sprite::Arc(arc)) = sprite_for(ch, W, HT) else {
                panic!("{ch} is not an arc");
            };
            assert_eq!(arc.corner, corner);
            assert!(approx(arc.thickness, 1.0));
            // The stroke's centreline radius is half the cell width, so the
            // outer radius is that plus half a stroke.
            assert!(approx(arc.radius, W / 2.0 + 0.5), "{ch}: {arc:?}");
            // The quad reaches the two cell edges the arms exit through, and
            // its stroked edges sit on the centre lines.
            let b = arc.bounds;
            match corner {
                Corner::TopLeft => {
                    assert!(approx(b.right(), W) && approx(b.bottom(), HT));
                    assert!(approx(b.x + 0.5, W / 2.0) && approx(b.y + 0.5, HT / 2.0));
                }
                Corner::TopRight => {
                    assert!(approx(b.x, 0.0) && approx(b.bottom(), HT));
                    assert!(approx(b.right() - 0.5, W / 2.0) && approx(b.y + 0.5, HT / 2.0));
                }
                Corner::BottomRight => {
                    assert!(approx(b.x, 0.0) && approx(b.y, 0.0));
                    assert!(approx(b.right() - 0.5, W / 2.0) && approx(b.bottom() - 0.5, HT / 2.0));
                }
                Corner::BottomLeft => {
                    assert!(approx(b.right(), W) && approx(b.y, 0.0));
                    assert!(approx(b.x + 0.5, W / 2.0) && approx(b.bottom() - 0.5, HT / 2.0));
                }
            }
            assert!(arc.radius <= b.w.min(b.h) + 1e-4, "{ch}: radius fits the quad");
        }
        assert_eq!(rects_for('╭', W, HT), None, "an arc is not a rect list");
    }

    #[test]
    fn strokes_scale_with_the_cell() {
        let small = rects_for('─', 6.0, 12.0).unwrap();
        let large = rects_for('─', 20.0, 40.0).unwrap();
        assert!(approx(small[0].h, 1.0), "never thinner than 1 px");
        assert!(large[0].h > small[0].h, "a 40 pt font gets a heavier line");
        // A degenerate cell still produces something finite.
        let broken = rects_for('█', 0.0, f32::NAN).unwrap();
        assert!(broken[0].w.is_finite() && broken[0].h.is_finite());
    }
}
