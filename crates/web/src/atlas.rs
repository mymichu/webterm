//! Glyph atlas: rasterizes glyphs with an offscreen 2D canvas and packs them
//! into a texture for the WebGL renderer.
//!
//! Text glyphs are drawn by the browser (so font fallback for CJK, emoji etc.
//! comes for free). Box drawing, block elements and Powerline separators are
//! drawn procedurally so that they line up pixel-perfectly across cells.

use std::collections::HashMap;
use std::f64::consts::PI;

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

pub const ATLAS_SIZE: u32 = 2048;

/// Cell geometry in device pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub cell_w: u32,
    pub cell_h: u32,
    pub baseline: f64,
    pub underline_y: u32,
    pub thickness: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub ch: char,
    pub bold: bool,
    pub italic: bool,
    pub wide: bool,
}

#[derive(Clone, Copy)]
pub struct Glyph {
    /// Normalized texture coordinates: u0, v0, u1, v1.
    pub uv: [f32; 4],
    pub width: u32,
    /// Color glyph (emoji): drawn as-is instead of tinted with the foreground.
    pub colored: bool,
    /// Nothing visible was drawn; skip it.
    pub empty: bool,
}

/// A rasterized glyph waiting to be copied into the GPU texture.
pub struct Upload {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct AtlasFull;

pub struct Atlas {
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    family: String,
    font_size: f64,
    size_px: f64,
    pub metrics: Metrics,
    glyphs: HashMap<GlyphKey, Glyph>,
    next_x: u32,
    next_y: u32,
    pub uploads: Vec<Upload>,
}

impl Atlas {
    pub fn new(family: &str, font_size: f64, dpr: f64) -> Result<Self, JsValue> {
        let document = web_sys::window().ok_or("no window")?.document().ok_or("no document")?;
        let canvas: HtmlCanvasElement = document.create_element("canvas")?.dyn_into()?;
        let opts = js_sys::Object::new();
        js_sys::Reflect::set(&opts, &"willReadFrequently".into(), &true.into())?;
        let ctx: CanvasRenderingContext2d = canvas.get_context_with_context_options("2d", &opts)?.ok_or("no 2d context")?.dyn_into()?;
        let mut atlas = Atlas {
            canvas,
            ctx,
            family: family.to_string(),
            font_size,
            size_px: 0.0,
            metrics: Metrics::default(),
            glyphs: HashMap::new(),
            next_x: 0,
            next_y: 0,
            uploads: Vec::new(),
        };
        atlas.set_scale(dpr)?;
        Ok(atlas)
    }

    /// Re-measures the font for a new device pixel ratio and drops all glyphs.
    pub fn set_scale(&mut self, dpr: f64) -> Result<(), JsValue> {
        self.size_px = (self.font_size * dpr).round().max(1.0);
        self.ctx.set_font(&self.font(false, false));
        let m = self.ctx.measure_text("M")?;
        let (mut ascent, mut descent) = (m.font_bounding_box_ascent(), m.font_bounding_box_descent());
        if !(ascent > 0.0) {
            ascent = self.size_px * 0.8;
            descent = self.size_px * 0.2;
        }
        let cell_w = m.width().round().max(1.0) as u32;
        let cell_h = (ascent + descent).ceil().max(1.0) as u32;
        let thickness = (self.size_px / 14.0).round().max(1.0) as u32;
        let baseline = ascent.round();
        let underline_y = ((baseline + (descent * 0.4).max(1.0)).round() as u32).min(cell_h - thickness);
        self.metrics = Metrics { cell_w, cell_h, baseline, underline_y, thickness };
        self.canvas.set_width(cell_w * 2);
        self.canvas.set_height(cell_h);
        self.reset();
        Ok(())
    }

    pub fn reset(&mut self) {
        self.glyphs.clear();
        self.uploads.clear();
        self.next_x = 0;
        self.next_y = 0;
    }

    fn font(&self, bold: bool, italic: bool) -> String {
        format!("{}{}{}px {}", if italic { "italic " } else { "" }, if bold { "bold " } else { "" }, self.size_px, self.family)
    }

    pub fn get(&mut self, key: GlyphKey) -> Result<Glyph, AtlasFull> {
        if let Some(glyph) = self.glyphs.get(&key) {
            return Ok(*glyph);
        }
        let glyph = self.rasterize(key)?;
        self.glyphs.insert(key, glyph);
        Ok(glyph)
    }

    fn rasterize(&mut self, key: GlyphKey) -> Result<Glyph, AtlasFull> {
        let Metrics { cell_w, cell_h, baseline, thickness, .. } = self.metrics;
        let (w, h) = (cell_w * if key.wide { 2 } else { 1 }, cell_h);

        // Shelf packing: all slots share the cell height.
        if self.next_x + w > ATLAS_SIZE {
            self.next_x = 0;
            self.next_y += h;
        }
        if self.next_y + h > ATLAS_SIZE {
            return Err(AtlasFull);
        }
        let (x, y) = (self.next_x, self.next_y);
        self.next_x += w;

        let ctx = &self.ctx;
        ctx.clear_rect(0.0, 0.0, self.canvas.width() as f64, self.canvas.height() as f64);
        ctx.set_fill_style_str("#fff");
        ctx.set_stroke_style_str("#fff");
        ctx.set_global_alpha(1.0);
        if !draw_special(ctx, key.ch, w as f64, h as f64, thickness as f64) {
            ctx.set_font(&self.font(key.bold, key.italic));
            let _ = ctx.fill_text(&key.ch.to_string(), 0.0, baseline);
        }
        let data = match ctx.get_image_data(0.0, 0.0, w as f64, h as f64) {
            Ok(img) => img.data().0,
            Err(_) => vec![0; (w * h * 4) as usize],
        };

        let mut empty = true;
        let mut colored = false;
        for px in data.chunks_exact(4) {
            if px[3] > 0 {
                empty = false;
                if px[0] != px[1] || px[1] != px[2] {
                    colored = true;
                    break;
                }
            }
        }
        if !empty {
            self.uploads.push(Upload { x, y, w, h, data });
        }
        let s = ATLAS_SIZE as f32;
        Ok(Glyph {
            uv: [x as f32 / s, y as f32 / s, (x + w) as f32 / s, (y + h) as f32 / s],
            width: w,
            colored,
            empty,
        })
    }
}

/// Line weights for the four arms of a box drawing character:
/// (up, right, down, left), 0 = none, 1 = light, 2 = heavy.
fn box_arms(c: char) -> Option<[u8; 4]> {
    Some(match c {
        '─' => [0, 1, 0, 1],
        '━' => [0, 2, 0, 2],
        '│' => [1, 0, 1, 0],
        '┃' => [2, 0, 2, 0],
        '┌' => [0, 1, 1, 0],
        '┍' => [0, 2, 1, 0],
        '┎' => [0, 1, 2, 0],
        '┏' => [0, 2, 2, 0],
        '┐' => [0, 0, 1, 1],
        '┑' => [0, 0, 1, 2],
        '┒' => [0, 0, 2, 1],
        '┓' => [0, 0, 2, 2],
        '└' => [1, 1, 0, 0],
        '┕' => [1, 2, 0, 0],
        '┖' => [2, 1, 0, 0],
        '┗' => [2, 2, 0, 0],
        '┘' => [1, 0, 0, 1],
        '┙' => [1, 0, 0, 2],
        '┚' => [2, 0, 0, 1],
        '┛' => [2, 0, 0, 2],
        '├' => [1, 1, 1, 0],
        '┝' => [1, 2, 1, 0],
        '┠' => [2, 1, 2, 0],
        '┣' => [2, 2, 2, 0],
        '┤' => [1, 0, 1, 1],
        '┥' => [1, 0, 1, 2],
        '┨' => [2, 0, 2, 1],
        '┫' => [2, 0, 2, 2],
        '┬' => [0, 1, 1, 1],
        '┯' => [0, 2, 1, 2],
        '┰' => [0, 1, 2, 1],
        '┳' => [0, 2, 2, 2],
        '┴' => [1, 1, 0, 1],
        '┷' => [1, 2, 0, 2],
        '┸' => [2, 1, 0, 1],
        '┻' => [2, 2, 0, 2],
        '┼' => [1, 1, 1, 1],
        '┿' => [1, 2, 1, 2],
        '╂' => [2, 1, 2, 1],
        '╋' => [2, 2, 2, 2],
        '╴' => [0, 0, 0, 1],
        '╵' => [1, 0, 0, 0],
        '╶' => [0, 1, 0, 0],
        '╷' => [0, 0, 1, 0],
        '╸' => [0, 0, 0, 2],
        '╹' => [2, 0, 0, 0],
        '╺' => [0, 2, 0, 0],
        '╻' => [0, 0, 2, 0],
        '╼' => [0, 2, 0, 1],
        '╽' => [1, 0, 2, 0],
        '╾' => [0, 1, 0, 2],
        '╿' => [2, 0, 1, 0],
        _ => return None,
    })
}

/// Draws characters that must tile seamlessly. Returns false if `c` should
/// be drawn with the font instead.
fn draw_special(ctx: &CanvasRenderingContext2d, c: char, w: f64, h: f64, t: f64) -> bool {
    let rect = |x: f64, y: f64, rw: f64, rh: f64| ctx.fill_rect(x.round(), y.round(), rw.round(), rh.round());

    if let Some(arms) = box_arms(c) {
        let (cx, cy) = ((w / 2.0).floor(), (h / 2.0).floor());
        let weight = |a: u8| if a == 2 { t * 2.0 } else { t };
        // Arms extend past the center by half the widest crossing line so joints are filled.
        let vert = weight(arms[0].max(arms[2]));
        let horiz = weight(arms[1].max(arms[3]));
        let [up, right, down, left] = arms;
        if up > 0 {
            let lw = weight(up);
            rect(cx - (lw / 2.0).floor(), 0.0, lw, cy + (horiz / 2.0).ceil());
        }
        if down > 0 {
            let lw = weight(down);
            let y0 = cy - (horiz / 2.0).floor();
            rect(cx - (lw / 2.0).floor(), y0, lw, h - y0);
        }
        if left > 0 {
            let lw = weight(left);
            rect(0.0, cy - (lw / 2.0).floor(), cx + (vert / 2.0).ceil(), lw);
        }
        if right > 0 {
            let lw = weight(right);
            let x0 = cx - (vert / 2.0).floor();
            rect(x0, cy - (lw / 2.0).floor(), w - x0, lw);
        }
        return true;
    }

    // Rounded corners.
    if let Some((up, right)) = match c {
        '╭' => Some((false, true)),
        '╮' => Some((false, false)),
        '╯' => Some((true, false)),
        '╰' => Some((true, true)),
        _ => None,
    } {
        let (cx, cy) = ((w / 2.0).floor() + (t / 2.0).fract(), (h / 2.0).floor() + (t / 2.0).fract());
        let r = cx.min(cy);
        let (ey, ex) = (if up { 0.0 } else { h }, if right { w } else { 0.0 });
        let sy = if up { -1.0 } else { 1.0 };
        let sx = if right { 1.0 } else { -1.0 };
        ctx.set_line_width(t);
        ctx.begin_path();
        ctx.move_to(cx, ey);
        ctx.line_to(cx, cy + sy * r);
        ctx.arc_to(cx, cy, cx + sx * r, cy, r).ok();
        ctx.line_to(ex, cy);
        ctx.stroke();
        return true;
    }

    match c as u32 {
        // Upper half, lower eighths, full block.
        0x2580 => rect(0.0, 0.0, w, h / 2.0),
        n @ 0x2581..=0x2588 => {
            let frac = (n - 0x2580) as f64 / 8.0;
            rect(0.0, h - h * frac, w, h * frac);
        }
        // Left eighths, right half.
        n @ 0x2589..=0x258f => rect(0.0, 0.0, w * (0x2590 - n) as f64 / 8.0, h),
        0x2590 => rect(w / 2.0, 0.0, w / 2.0, h),
        // Shades.
        n @ 0x2591..=0x2593 => {
            ctx.set_global_alpha((n - 0x2590) as f64 * 0.25);
            rect(0.0, 0.0, w, h);
            ctx.set_global_alpha(1.0);
        }
        0x2594 => rect(0.0, 0.0, w, h / 8.0),
        0x2595 => rect(w - w / 8.0, 0.0, w / 8.0, h),
        // Quadrants: bits are upper-left, upper-right, lower-left, lower-right.
        n @ 0x2596..=0x259f => {
            const QUADS: [u8; 10] = [0b0100, 0b1000, 0b0001, 0b1101, 0b1001, 0b0111, 0b1011, 0b0010, 0b0110, 0b1110];
            let q = QUADS[(n - 0x2596) as usize];
            let (hw, hh) = ((w / 2.0).round(), (h / 2.0).round());
            if q & 1 != 0 { rect(0.0, 0.0, hw, hh) }
            if q & 2 != 0 { rect(hw, 0.0, w - hw, hh) }
            if q & 4 != 0 { rect(0.0, hh, hw, h - hh) }
            if q & 8 != 0 { rect(hw, hh, w - hw, h - hh) }
        }
        // Powerline separators.
        0xe0b0 | 0xe0b2 => {
            let (a, b) = if c as u32 == 0xe0b0 { (0.0, w) } else { (w, 0.0) };
            ctx.begin_path();
            ctx.move_to(a, 0.0);
            ctx.line_to(b, h / 2.0);
            ctx.line_to(a, h);
            ctx.close_path();
            ctx.fill();
        }
        0xe0b1 | 0xe0b3 => {
            let (a, b) = if c as u32 == 0xe0b1 { (0.0, w - t) } else { (w, t) };
            ctx.set_line_width(t);
            ctx.begin_path();
            ctx.move_to(a, 0.0);
            ctx.line_to(b, h / 2.0);
            ctx.line_to(a, h);
            ctx.stroke();
        }
        0xe0b4 | 0xe0b6 => {
            let (x, start) = if c as u32 == 0xe0b4 { (0.0, -PI / 2.0) } else { (w, PI / 2.0) };
            ctx.begin_path();
            ctx.ellipse(x, h / 2.0, w, h / 2.0, 0.0, start, start + PI).ok();
            ctx.fill();
        }
        _ => return false,
    }
    true
}
