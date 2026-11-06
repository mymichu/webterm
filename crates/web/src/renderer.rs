//! GPU renderer: draws the terminal grid with WebGL2.
//!
//! Every visible element (cell background, glyph, underline, cursor) is one
//! instance of a unit quad, so a whole frame is a single instanced draw call.
//! Instances are drawn in order: backgrounds, then text, then the cursor.

use term_core::{CursorShape, Flags, Palette, Rgb, Terminal};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{HtmlCanvasElement, WebGl2RenderingContext as GL, WebGlProgram, WebGlShader, WebGlTexture, WebGlUniformLocation};

use crate::atlas::{ATLAS_SIZE, Atlas, AtlasFull, GlyphKey, Metrics};

const VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec2 a_corner;
layout(location = 1) in vec4 a_rect;
layout(location = 2) in vec4 a_uv;
layout(location = 3) in vec4 a_color;
layout(location = 4) in float a_mode;
uniform vec2 u_resolution;
out vec2 v_uv;
out vec4 v_color;
flat out int v_mode;
void main() {
    vec2 pos = a_rect.xy + a_corner * a_rect.zw;
    vec2 clip = pos / u_resolution * 2.0 - 1.0;
    gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);
    v_uv = mix(a_uv.xy, a_uv.zw, a_corner);
    v_color = a_color;
    v_mode = int(a_mode);
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 300 es
precision mediump float;
uniform sampler2D u_atlas;
in vec2 v_uv;
in vec4 v_color;
flat in int v_mode;
out vec4 out_color;
void main() {
    if (v_mode == 0) {
        out_color = v_color;                       // solid rectangle
    } else {
        vec4 texel = texture(u_atlas, v_uv);
        if (v_mode == 1) {
            out_color = vec4(v_color.rgb, v_color.a * texel.a);  // tinted glyph mask
        } else {
            out_color = texel;                     // color glyph (emoji)
        }
    }
}
"#;

/// Floats per instance: rect(4) + uv(4) + color(4) + mode(1).
const STRIDE: usize = 13;
const MODE_SOLID: f32 = 0.0;
const MODE_MASK: f32 = 1.0;
const MODE_COLOR: f32 = 2.0;

pub struct ViewState {
    pub focused: bool,
    pub cursor_blink_on: bool,
    /// Grid offset from the canvas edge, in device pixels.
    pub padding: u32,
}

pub struct Renderer {
    gl: GL,
    program: WebGlProgram,
    instance_buffer: web_sys::WebGlBuffer,
    texture: WebGlTexture,
    u_resolution: Option<WebGlUniformLocation>,
    pub atlas: Atlas,
    width: u32,
    height: u32,
    backgrounds: Vec<f32>,
    foregrounds: Vec<f32>,
    overlay: Vec<f32>,
}

fn rgba(c: Rgb, alpha: f32) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, alpha]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| (a[i] as f32 * (1.0 - t) + b[i] as f32 * t).round() as u8)
}

fn push(buf: &mut Vec<f32>, rect: [f32; 4], uv: [f32; 4], color: [f32; 4], mode: f32) {
    buf.extend_from_slice(&rect);
    buf.extend_from_slice(&uv);
    buf.extend_from_slice(&color);
    buf.push(mode);
}

fn push_rect(buf: &mut Vec<f32>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
    push(buf, [x, y, w, h], [0.0; 4], color, MODE_SOLID);
}

impl Renderer {
    pub fn new(canvas: &HtmlCanvasElement, atlas: Atlas) -> Result<Self, JsValue> {
        let opts = js_sys::Object::new();
        js_sys::Reflect::set(&opts, &"alpha".into(), &false.into())?;
        js_sys::Reflect::set(&opts, &"antialias".into(), &false.into())?;
        let gl: GL = canvas
            .get_context_with_context_options("webgl2", &opts)?
            .ok_or("WebGL2 is not supported by this browser")?
            .dyn_into()?;

        let program = link_program(&gl, VERTEX_SHADER, FRAGMENT_SHADER)?;
        gl.use_program(Some(&program));
        let u_resolution = gl.get_uniform_location(&program, "u_resolution");
        gl.uniform1i(gl.get_uniform_location(&program, "u_atlas").as_ref(), 0);

        let vao = gl.create_vertex_array().ok_or("failed to create VAO")?;
        gl.bind_vertex_array(Some(&vao));

        // Unit quad as two triangles.
        let quad_buffer = gl.create_buffer().ok_or("failed to create buffer")?;
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&quad_buffer));
        let quad: [f32; 12] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0];
        // SAFETY: the view is consumed by buffer_data before any allocation.
        unsafe {
            gl.buffer_data_with_array_buffer_view(GL::ARRAY_BUFFER, &js_sys::Float32Array::view(&quad), GL::STATIC_DRAW);
        }
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 2, GL::FLOAT, false, 0, 0);

        let instance_buffer = gl.create_buffer().ok_or("failed to create buffer")?;
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&instance_buffer));
        let stride = (STRIDE * 4) as i32;
        for (loc, size, offset) in [(1, 4, 0), (2, 4, 16), (3, 4, 32), (4, 1, 48)] {
            gl.enable_vertex_attrib_array(loc);
            gl.vertex_attrib_pointer_with_i32(loc, size, GL::FLOAT, false, stride, offset);
            gl.vertex_attrib_divisor(loc, 1);
        }

        let texture = gl.create_texture().ok_or("failed to create texture")?;
        gl.active_texture(GL::TEXTURE0);
        gl.bind_texture(GL::TEXTURE_2D, Some(&texture));
        for (param, value) in [
            (GL::TEXTURE_MIN_FILTER, GL::NEAREST),
            (GL::TEXTURE_MAG_FILTER, GL::NEAREST),
            (GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE),
            (GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameteri(GL::TEXTURE_2D, param, value as i32);
        }
        gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
            GL::TEXTURE_2D, 0, GL::RGBA as i32, ATLAS_SIZE as i32, ATLAS_SIZE as i32, 0, GL::RGBA, GL::UNSIGNED_BYTE, None,
        )?;

        gl.enable(GL::BLEND);
        gl.blend_func(GL::SRC_ALPHA, GL::ONE_MINUS_SRC_ALPHA);

        Ok(Renderer {
            gl,
            program,
            instance_buffer,
            texture,
            u_resolution,
            atlas,
            width: 1,
            height: 1,
            backgrounds: Vec::new(),
            foregrounds: Vec::new(),
            overlay: Vec::new(),
        })
    }

    pub fn metrics(&self) -> Metrics {
        self.atlas.metrics
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
    }

    pub fn render(&mut self, term: &Terminal, view: &ViewState) {
        if self.build(term, view).is_err() {
            // The atlas filled up mid-frame: start over with an empty atlas.
            self.atlas.reset();
            let _ = self.build(term, view);
        }
        self.upload_glyphs();

        let gl = &self.gl;
        let bg = rgba(term.palette().background, 1.0);
        gl.viewport(0, 0, self.width as i32, self.height as i32);
        gl.clear_color(bg[0], bg[1], bg[2], 1.0);
        gl.clear(GL::COLOR_BUFFER_BIT);

        self.backgrounds.extend_from_slice(&self.foregrounds);
        self.backgrounds.extend_from_slice(&self.overlay);
        let instances = &self.backgrounds;
        if instances.is_empty() {
            return;
        }
        gl.use_program(Some(&self.program));
        gl.uniform2f(self.u_resolution.as_ref(), self.width as f32, self.height as f32);
        gl.bind_texture(GL::TEXTURE_2D, Some(&self.texture));
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.instance_buffer));
        // SAFETY: the view is consumed by buffer_data before any allocation.
        unsafe {
            gl.buffer_data_with_array_buffer_view(GL::ARRAY_BUFFER, &js_sys::Float32Array::view(instances), GL::STREAM_DRAW);
        }
        gl.draw_arrays_instanced(GL::TRIANGLES, 0, 6, (instances.len() / STRIDE) as i32);
    }

    fn upload_glyphs(&mut self) {
        for up in self.atlas.uploads.drain(..) {
            let _ = self.gl.tex_sub_image_2d_with_i32_and_i32_and_u32_and_type_and_opt_u8_array(
                GL::TEXTURE_2D, 0, up.x as i32, up.y as i32, up.w as i32, up.h as i32, GL::RGBA, GL::UNSIGNED_BYTE, Some(&up.data),
            );
        }
    }

    fn build(&mut self, term: &Terminal, view: &ViewState) -> Result<(), AtlasFull> {
        self.backgrounds.clear();
        self.foregrounds.clear();
        self.overlay.clear();

        let palette: &Palette = term.palette();
        let Metrics { cell_w, cell_h, underline_y, thickness, .. } = self.atlas.metrics;
        let (cw, ch, t) = (cell_w as f32, cell_h as f32, thickness as f32);
        let pad = view.padding as f32;
        let selection = term.selection_range();
        let cols = term.cols();

        for y in 0..term.rows() {
            let row = term.viewport_row(y);
            let line = term.viewport_line_id(y);
            let py = pad + y as f32 * ch;
            let mut x = 0;
            while x < cols {
                let cell = row.get(x);
                if cell.flags.contains(Flags::SPACER) {
                    x += 1;
                    continue;
                }
                let span = if cell.flags.contains(Flags::WIDE) { 2 } else { 1 };
                let px = pad + x as f32 * cw;
                let width = cw * span as f32;

                let (mut fg, mut bg) = (palette.resolve(cell.fg, true), palette.resolve(cell.bg, false));
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if selection.is_some_and(|s| s.contains(line, x)) {
                    bg = palette.selection;
                }
                if cell.flags.contains(Flags::DIM) {
                    fg = mix(fg, bg, 0.5);
                }
                if bg != palette.background {
                    push_rect(&mut self.backgrounds, px, py, width, ch, rgba(bg, 1.0));
                }
                if cell.flags.contains(Flags::HIDDEN) {
                    x += span;
                    continue;
                }

                let color = rgba(fg, 1.0);
                if cell.ch != ' ' {
                    let key = GlyphKey {
                        ch: cell.ch,
                        bold: cell.flags.contains(Flags::BOLD),
                        italic: cell.flags.contains(Flags::ITALIC),
                        wide: span == 2,
                    };
                    let glyph = self.atlas.get(key)?;
                    if !glyph.empty {
                        let mode = if glyph.colored { MODE_COLOR } else { MODE_MASK };
                        push(&mut self.foregrounds, [px, py, glyph.width as f32, ch], glyph.uv, color, mode);
                    }
                }

                let uy = py + underline_y as f32;
                if cell.flags.contains(Flags::UNDERLINE) {
                    push_rect(&mut self.foregrounds, px, uy, width, t, color);
                }
                if cell.flags.contains(Flags::DOUBLE_UNDERLINE) {
                    push_rect(&mut self.foregrounds, px, uy - t, width, t, color);
                    push_rect(&mut self.foregrounds, px, uy + t, width, t, color);
                }
                if cell.flags.contains(Flags::CURLY_UNDERLINE) {
                    let amp = t.max(1.0);
                    let period = cw.max(4.0);
                    for i in 0..width as u32 {
                        let phase = ((x as f32 * cw + i as f32) / period) * std::f32::consts::TAU;
                        let dy = (phase.sin() * amp).round();
                        push_rect(&mut self.foregrounds, px + i as f32, uy + dy, 1.0, t, color);
                    }
                }
                if cell.flags.contains(Flags::STRIKE) {
                    push_rect(&mut self.foregrounds, px, py + (ch / 2.0).round(), width, t, color);
                }
                x += span;
            }
        }

        self.build_cursor(term, view)
    }

    fn build_cursor(&mut self, term: &Terminal, view: &ViewState) -> Result<(), AtlasFull> {
        let cursor = term.cursor();
        let Some(cy) = cursor.y else { return Ok(()) };
        if view.focused && cursor.blink && !view.cursor_blink_on {
            return Ok(());
        }
        let palette = term.palette();
        let Metrics { cell_w, cell_h, thickness, .. } = self.atlas.metrics;
        let (cw, ch, t) = (cell_w as f32, cell_h as f32, thickness as f32);
        let cell = term.viewport_row(cy).get(cursor.x);
        let span = if cell.flags.contains(Flags::WIDE) { 2.0 } else { 1.0 };
        let px = view.padding as f32 + cursor.x as f32 * cw;
        let py = view.padding as f32 + cy as f32 * ch;
        let w = cw * span;
        let color = rgba(palette.cursor, 1.0);

        if !view.focused {
            // Hollow block when the terminal doesn't have focus.
            push_rect(&mut self.overlay, px, py, w, t, color);
            push_rect(&mut self.overlay, px, py + ch - t, w, t, color);
            push_rect(&mut self.overlay, px, py, t, ch, color);
            push_rect(&mut self.overlay, px + w - t, py, t, ch, color);
            return Ok(());
        }
        match cursor.shape {
            CursorShape::Block => {
                push_rect(&mut self.overlay, px, py, w, ch, color);
                // Redraw the glyph under the cursor in the background color.
                if cell.ch != ' ' && !cell.flags.contains(Flags::HIDDEN) {
                    let glyph = self.atlas.get(GlyphKey {
                        ch: cell.ch,
                        bold: cell.flags.contains(Flags::BOLD),
                        italic: cell.flags.contains(Flags::ITALIC),
                        wide: span == 2.0,
                    })?;
                    if !glyph.empty && !glyph.colored {
                        push(&mut self.overlay, [px, py, glyph.width as f32, ch], glyph.uv, rgba(palette.background, 1.0), MODE_MASK);
                    }
                }
            }
            CursorShape::Bar => push_rect(&mut self.overlay, px, py, (t * 2.0).max(2.0), ch, color),
            CursorShape::Underline => push_rect(&mut self.overlay, px, py + ch - t * 2.0, w, t * 2.0, color),
        }
        Ok(())
    }
}

fn compile_shader(gl: &GL, kind: u32, source: &str) -> Result<WebGlShader, JsValue> {
    let shader = gl.create_shader(kind).ok_or("failed to create shader")?;
    gl.shader_source(&shader, source);
    gl.compile_shader(&shader);
    if gl.get_shader_parameter(&shader, GL::COMPILE_STATUS).as_bool() == Some(true) {
        Ok(shader)
    } else {
        Err(gl.get_shader_info_log(&shader).unwrap_or_default().into())
    }
}

fn link_program(gl: &GL, vertex: &str, fragment: &str) -> Result<WebGlProgram, JsValue> {
    let program = gl.create_program().ok_or("failed to create program")?;
    gl.attach_shader(&program, &compile_shader(gl, GL::VERTEX_SHADER, vertex)?);
    gl.attach_shader(&program, &compile_shader(gl, GL::FRAGMENT_SHADER, fragment)?);
    gl.link_program(&program);
    if gl.get_program_parameter(&program, GL::LINK_STATUS).as_bool() == Some(true) {
        Ok(program)
    } else {
        Err(gl.get_program_info_log(&program).unwrap_or_default().into())
    }
}
