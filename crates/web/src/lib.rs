//! Browser front end: wires the terminal core to the DOM, a WebGL renderer
//! and a WebSocket connection to the backend.

mod atlas;
mod renderer;

use std::cell::RefCell;
use std::rc::Rc;

use protocol::{ClientMsg, ServerMsg};
use term_core::input::{Key, Mods, MouseAction, MouseButton, encode_key, encode_mouse};
use term_core::{MouseMode, SelectionKind, Terminal};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    BinaryType, ClipboardEvent, CloseEvent, Event, HtmlCanvasElement, HtmlElement, HtmlTextAreaElement, KeyboardEvent, MessageEvent,
    MouseEvent, ResizeObserver, WebSocket, WheelEvent,
};

use crate::atlas::Atlas;
use crate::renderer::{Renderer, ViewState};

const CURSOR_BLINK_MS: f64 = 600.0;
/// Give up waiting for the end of a synchronized update after this long.
const SYNC_TIMEOUT_MS: f64 = 1000.0;
const PADDING_CSS_PX: f64 = 8.0;

type Shared = Rc<RefCell<App>>;

struct App {
    term: Terminal,
    renderer: Renderer,
    container: HtmlElement,
    canvas: HtmlCanvasElement,
    textarea: HtmlTextAreaElement,
    ws_url: String,
    ws: Option<WebSocket>,
    ws_handlers: Vec<Closure<dyn FnMut(JsValue)>>,
    connected: bool,
    /// Set by the key handler; the frame loop reconnects (it owns the `Rc`).
    reconnect: bool,
    focused: bool,
    dpr: f64,
    blink_on: bool,
    last_blink: f64,
    sync_since: Option<f64>,
    /// Mouse button held down while the application tracks the mouse.
    reported_button: Option<MouseButton>,
    last_mouse_cell: Option<(usize, usize)>,
    selecting: bool,
    wheel_accum: f64,
}

/// Handle returned to JavaScript.
#[wasm_bindgen]
pub struct TerminalHandle {
    app: Shared,
}

#[wasm_bindgen]
impl TerminalHandle {
    pub fn focus(&self) {
        let textarea = self.app.borrow().textarea.clone();
        let _ = textarea.focus();
    }

    /// Writes text to the terminal display (not to the shell), e.g. for status messages.
    pub fn write(&self, text: &str) {
        with_app(&self.app, |app| app.term.feed(text.as_bytes()));
    }
}

/// Creates a terminal inside `container` and connects it to `ws_url`.
#[wasm_bindgen]
pub fn start(container: HtmlElement, ws_url: String, font_family: String, font_size: f64) -> Result<TerminalHandle, JsValue> {
    console_error_panic_hook::set_once();
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    let canvas: HtmlCanvasElement = document.create_element("canvas")?.dyn_into()?;
    canvas.style().set_css_text("display:block;width:100%;height:100%;cursor:text;");
    let textarea: HtmlTextAreaElement = document.create_element("textarea")?.dyn_into()?;
    for (name, value) in [("autocapitalize", "off"), ("autocomplete", "off"), ("autocorrect", "off"), ("spellcheck", "false"), ("aria-label", "Terminal input"), ("tabindex", "0")] {
        textarea.set_attribute(name, value)?;
    }
    textarea.style().set_css_text(
        "position:absolute;left:0;top:0;width:1px;height:1em;opacity:0;padding:0;border:0;margin:0;resize:none;overflow:hidden;white-space:pre;z-index:-1;",
    );
    // The textarea is absolutely positioned, so the container must be a containing block.
    let position = window.get_computed_style(&container)?.map(|s| s.get_property_value("position")).transpose()?;
    if position.as_deref().is_none_or(|p| p == "static") {
        container.style().set_property("position", "relative")?;
    }
    container.append_child(&canvas)?;
    container.append_child(&textarea)?;

    let dpr = window.device_pixel_ratio();
    let atlas = Atlas::new(&font_family, font_size, dpr)?;
    let renderer = Renderer::new(&canvas, atlas)?;

    let app = Rc::new(RefCell::new(App {
        term: Terminal::new(80, 24),
        renderer,
        container,
        canvas,
        textarea,
        ws_url,
        ws: None,
        ws_handlers: Vec::new(),
        connected: false,
        reconnect: false,
        focused: false,
        dpr,
        blink_on: true,
        last_blink: 0.0,
        sync_since: None,
        reported_button: None,
        last_mouse_cell: None,
        selecting: false,
        wheel_accum: 0.0,
    }));

    app.borrow_mut().fit();
    install_resize_handlers(&app)?;
    install_keyboard_handlers(&app)?;
    install_mouse_handlers(&app)?;
    connect(&app)?;
    start_render_loop(&app);
    Ok(TerminalHandle { app })
}

/// Runs `f` with the app unless it is already borrowed (re-entrant DOM event).
fn with_app<R>(app: &Shared, f: impl FnOnce(&mut App) -> R) -> Option<R> {
    app.try_borrow_mut().ok().map(|mut app| f(&mut app))
}

fn listen<E: JsCast + 'static>(target: &web_sys::EventTarget, event: &str, app: &Shared, mut f: impl FnMut(&mut App, E) + 'static) -> Result<(), JsValue> {
    let app = app.clone();
    let closure = Closure::<dyn FnMut(Event)>::new(move |e: Event| {
        with_app(&app, |app| f(app, e.unchecked_into()));
    });
    target.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref())?;
    closure.forget();
    Ok(())
}

impl App {
    fn send(&self, msg: ClientMsg) {
        if let (Some(ws), true) = (&self.ws, self.connected) {
            let _ = ws.send_with_u8_array(&msg.encode());
        }
    }

    fn send_input(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.send(ClientMsg::Input(bytes));
        }
    }

    /// Typing snaps back to the live screen and keeps the cursor visible.
    fn on_user_input(&mut self) {
        self.term.selection_clear();
        self.term.scroll_to_bottom();
        self.blink_on = true;
        self.last_blink = now();
        self.term.mark_dirty();
    }

    fn padding(&self) -> u32 {
        (PADDING_CSS_PX * self.dpr).round() as u32
    }

    /// Sizes the canvas to its container and the grid to the canvas.
    fn fit(&mut self) {
        let Some(window) = web_sys::window() else { return };
        let dpr = window.device_pixel_ratio();
        if dpr != self.dpr {
            self.dpr = dpr;
            let _ = self.renderer.atlas.set_scale(dpr);
        }
        let rect = self.container.get_bounding_client_rect();
        let (w, h) = ((rect.width() * dpr).round() as u32, (rect.height() * dpr).round() as u32);
        self.canvas.set_width(w.max(1));
        self.canvas.set_height(h.max(1));
        self.renderer.resize(w, h);

        let m = self.renderer.metrics();
        let pad = self.padding() * 2;
        let cols = (w.saturating_sub(pad) / m.cell_w).max(1) as usize;
        let rows = (h.saturating_sub(pad) / m.cell_h).max(1) as usize;
        if (cols, rows) != (self.term.cols(), self.term.rows()) {
            self.term.resize(cols, rows);
            self.send(ClientMsg::Resize { cols: cols as u16, rows: rows as u16 });
        }
        self.term.mark_dirty();
    }

    /// Converts a mouse position to a (col, row) cell, clamped to the grid.
    fn cell_at(&self, e: &MouseEvent) -> (usize, usize) {
        let rect = self.canvas.get_bounding_client_rect();
        let m = self.renderer.metrics();
        let pad = self.padding() as f64;
        let x = (e.client_x() as f64 - rect.left()) * self.dpr - pad;
        let y = (e.client_y() as f64 - rect.top()) * self.dpr - pad;
        let col = (x / m.cell_w as f64).floor().clamp(0.0, (self.term.cols() - 1) as f64) as usize;
        let row = (y / m.cell_h as f64).floor().clamp(0.0, (self.term.rows() - 1) as f64) as usize;
        (col, row)
    }

    fn mouse_reporting(&self, e: &MouseEvent) -> bool {
        // Shift bypasses mouse reporting so text can still be selected.
        self.term.modes().mouse != MouseMode::Off && !e.shift_key()
    }

    fn report_mouse(&self, button: MouseButton, action: MouseAction, (col, row): (usize, usize), e: &MouseEvent) {
        let mods = Mods { shift: e.shift_key(), alt: e.alt_key(), ctrl: e.ctrl_key() };
        if let Some(bytes) = encode_mouse(button, action, col, row, mods, self.term.modes()) {
            self.send_input(bytes);
        }
    }

    fn copy_selection(&self) {
        if let Some(text) = self.term.selection_text() {
            write_clipboard(&text);
        }
    }

    /// Applies everything the core produced after processing output.
    fn after_output(&mut self) {
        let responses = self.term.take_responses();
        self.send_input(responses);
        if let Some(title) = self.term.take_title()
            && let Some(doc) = web_sys::window().and_then(|w| w.document())
        {
            doc.set_title(if title.is_empty() { "webterm" } else { &title });
        }
        if let Some(text) = self.term.take_clipboard() {
            write_clipboard(&text);
        }
        self.term.take_bell();
    }

    fn render(&mut self) {
        let view = ViewState { focused: self.focused, cursor_blink_on: self.blink_on, padding: self.padding() };
        self.renderer.render(&self.term, &view);

        // Keep the hidden textarea at the cursor so IME popups appear there.
        if let Some(y) = self.term.cursor().y {
            let m = self.renderer.metrics();
            let pad = self.padding() as f64;
            let left = (pad + self.term.cursor().x as f64 * m.cell_w as f64) / self.dpr;
            let top = (pad + y as f64 * m.cell_h as f64) / self.dpr;
            let style = self.textarea.style();
            let _ = style.set_property("left", &format!("{left}px"));
            let _ = style.set_property("top", &format!("{top}px"));
        }
    }
}

fn now() -> f64 {
    js_sys::Date::now()
}

fn write_clipboard(text: &str) {
    let Some(window) = web_sys::window() else { return };
    let navigator = window.navigator();
    let Ok(clipboard) = js_sys::Reflect::get(&navigator, &"clipboard".into()) else { return };
    if clipboard.is_undefined() {
        return;
    }
    if let Ok(write) = js_sys::Reflect::get(&clipboard, &"writeText".into()).and_then(|f| f.dyn_into::<js_sys::Function>()) {
        let _ = write.call1(&clipboard, &text.into());
    }
}

// ---- WebSocket -------------------------------------------------------------

fn connect(app: &Shared) -> Result<(), JsValue> {
    let url = app.borrow().ws_url.clone();
    let ws = WebSocket::new(&url)?;
    ws.set_binary_type(BinaryType::Arraybuffer);

    let on_open = {
        let app = app.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |_| {
            with_app(&app, |app| {
                app.connected = true;
                let (cols, rows) = (app.term.cols() as u16, app.term.rows() as u16);
                app.send(ClientMsg::Resize { cols, rows });
            });
        })
    };
    let on_message = {
        let app = app.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let e: MessageEvent = e.unchecked_into();
            let Ok(buf) = e.data().dyn_into::<js_sys::ArrayBuffer>() else { return };
            let frame = js_sys::Uint8Array::new(&buf).to_vec();
            with_app(&app, |app| match ServerMsg::decode(&frame) {
                Ok(ServerMsg::Output(bytes)) => {
                    app.term.feed(&bytes);
                    app.after_output();
                }
                Ok(ServerMsg::Exit(code)) => {
                    app.term.feed(format!("\r\n\x1b[0;2m[process exited with code {code}]\x1b[0m").as_bytes());
                }
                Err(_) => {}
            });
        })
    };
    let on_close = {
        let app = app.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            let e: CloseEvent = e.unchecked_into();
            with_app(&app, |app| {
                app.connected = false;
                let reason = if e.reason().is_empty() { String::new() } else { format!(": {}", e.reason()) };
                app.term.feed(format!("\r\n\x1b[0;2m[disconnected{reason} \u{2014} press Enter to reconnect]\x1b[0m").as_bytes());
            });
        })
    };
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));

    let mut a = app.borrow_mut();
    if let Some(old) = a.ws.take() {
        old.set_onopen(None);
        old.set_onmessage(None);
        old.set_onclose(None);
        let _ = old.close();
    }
    a.ws = Some(ws);
    a.ws_handlers = vec![on_open, on_message, on_close];
    Ok(())
}

// ---- Rendering & resizing ----------------------------------------------------

fn start_render_loop(app: &Shared) {
    let callback: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
    let next = callback.clone();
    let app = app.clone();
    *callback.borrow_mut() = Some(Closure::new(move |_ts: f64| {
        let reconnect = with_app(&app, |app| {
            let t = now();
            if app.focused && app.term.modes().cursor_blink && t - app.last_blink >= CURSOR_BLINK_MS {
                app.blink_on = !app.blink_on;
                app.last_blink = t;
                app.term.mark_dirty();
            }
            // Synchronized output (mode 2026): hold the frame until the app is done drawing.
            let hold = if app.term.modes().synchronized_output {
                let since = *app.sync_since.get_or_insert(t);
                t - since < SYNC_TIMEOUT_MS
            } else {
                app.sync_since = None;
                false
            };
            if !hold && app.term.take_dirty() {
                app.render();
            }
            std::mem::take(&mut app.reconnect)
        });
        if reconnect == Some(true) {
            let _ = connect(&app);
        }
        request_frame(next.borrow().as_ref().expect("frame callback"));
    }));
    request_frame(callback.borrow().as_ref().expect("frame callback"));
}

fn request_frame(f: &Closure<dyn FnMut(f64)>) {
    if let Some(window) = web_sys::window() {
        let _ = window.request_animation_frame(f.as_ref().unchecked_ref());
    }
}

fn install_resize_handlers(app: &Shared) -> Result<(), JsValue> {
    let observer_cb = {
        let app = app.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |_| {
            with_app(&app, |app| app.fit());
        })
    };
    let observer = ResizeObserver::new(observer_cb.as_ref().unchecked_ref())?;
    observer.observe(&app.borrow().container);
    observer_cb.forget();

    // Device pixel ratio changes (zoom, moving between displays) fire window resize.
    let window = web_sys::window().ok_or("no window")?;
    listen::<Event>(&window, "resize", app, |app, _| app.fit())
}

// ---- Keyboard, text input, clipboard ---------------------------------------

fn dom_key(key: &str) -> Option<Key> {
    Some(match key {
        "Enter" => Key::Enter,
        "Backspace" => Key::Backspace,
        "Tab" => Key::Tab,
        "Escape" => Key::Escape,
        "ArrowUp" => Key::Up,
        "ArrowDown" => Key::Down,
        "ArrowLeft" => Key::Left,
        "ArrowRight" => Key::Right,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "Insert" => Key::Insert,
        "Delete" => Key::Delete,
        k if k.len() > 1 && k.starts_with('F') => Key::F(k[1..].parse().ok()?),
        k => {
            let mut chars = k.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Key::Char(c)
        }
    })
}

fn install_keyboard_handlers(app: &Shared) -> Result<(), JsValue> {
    let textarea = app.borrow().textarea.clone();

    listen::<KeyboardEvent>(&textarea, "keydown", app, |app, e| {
        if e.is_composing() || e.key_code() == 229 {
            return;
        }
        // Leave Cmd shortcuts (copy, paste, ...) to the browser.
        if e.meta_key() {
            return;
        }
        let key = e.key();
        let (shift, alt, ctrl) = (e.shift_key(), e.alt_key(), e.ctrl_key());

        if ctrl && shift && key.eq_ignore_ascii_case("c") {
            app.copy_selection();
            e.prevent_default();
            return;
        }
        if ctrl && shift && key.eq_ignore_ascii_case("v") {
            return; // the browser fires a paste event
        }
        if shift && !ctrl && !alt && !app.term.is_alt_screen() {
            let page = app.term.rows() as isize - 1;
            let delta = match key.as_str() {
                "PageUp" => Some(page),
                "PageDown" => Some(-page),
                "Home" => Some(isize::MAX / 2),
                "End" => Some(isize::MIN / 2),
                _ => None,
            };
            if let Some(delta) = delta {
                app.term.scroll_viewport(delta);
                e.prevent_default();
                return;
            }
        }

        let Some(k) = dom_key(&key) else { return };
        if !app.connected {
            if k == Key::Enter {
                e.prevent_default();
                app.term.feed(b"\r\n");
                app.reconnect = true;
            }
            return;
        }
        // Plain text goes through the `input` event so keyboard layouts, dead
        // keys and IME composition work. Non-ASCII with Alt is a composed
        // character (e.g. macOS Option), also treated as text.
        if let Key::Char(c) = k
            && !ctrl
            && (!alt || !c.is_ascii())
        {
            return;
        }
        let mods = Mods { shift, alt, ctrl };
        if let Some(bytes) = encode_key(k, mods, app.term.modes()) {
            e.prevent_default();
            app.send_input(bytes);
            app.on_user_input();
        }
    })?;

    listen::<Event>(&textarea, "input", app, |app, e| {
        if js_sys::Reflect::get(&e, &"isComposing".into()).ok().and_then(|v| v.as_bool()) == Some(true) {
            return;
        }
        let text = app.textarea.value();
        app.textarea.set_value("");
        if !text.is_empty() && app.connected {
            app.send_input(text.replace('\n', "\r").into_bytes());
            app.on_user_input();
        }
    })?;

    listen::<Event>(&textarea, "compositionend", app, |app, _| {
        let text = app.textarea.value();
        app.textarea.set_value("");
        if !text.is_empty() {
            app.send_input(text.into_bytes());
            app.on_user_input();
        }
    })?;

    listen::<ClipboardEvent>(&textarea, "paste", app, |app, e| {
        e.prevent_default();
        let Some(text) = e.clipboard_data().and_then(|d| d.get_data("text/plain").ok()) else { return };
        let mut text = text.replace("\r\n", "\r").replace('\n', "\r");
        if app.term.modes().bracketed_paste {
            // Strip any embedded end marker so pasted text can't escape the bracket.
            text = format!("\x1b[200~{}\x1b[201~", text.replace("\x1b[201~", ""));
        }
        app.send_input(text.into_bytes());
        app.on_user_input();
    })?;

    listen::<ClipboardEvent>(&textarea, "copy", app, |app, e| {
        if let (Some(text), Some(data)) = (app.term.selection_text(), e.clipboard_data()) {
            let _ = data.set_data("text/plain", &text);
            e.prevent_default();
        }
    })?;

    listen::<Event>(&textarea, "focus", app, |app, _| {
        app.focused = true;
        app.blink_on = true;
        app.term.mark_dirty();
        if app.term.modes().focus_events {
            app.send_input(b"\x1b[I".to_vec());
        }
    })?;

    listen::<Event>(&textarea, "blur", app, |app, _| {
        app.focused = false;
        app.term.mark_dirty();
        if app.term.modes().focus_events {
            app.send_input(b"\x1b[O".to_vec());
        }
    })
}

// ---- Mouse -------------------------------------------------------------------

fn mouse_button(e: &MouseEvent) -> MouseButton {
    match e.button() {
        0 => MouseButton::Left,
        1 => MouseButton::Middle,
        _ => MouseButton::Right,
    }
}

fn install_mouse_handlers(app: &Shared) -> Result<(), JsValue> {
    let canvas = app.borrow().canvas.clone();
    let window = web_sys::window().ok_or("no window")?;

    // Focus is moved to the textarea outside of the app borrow, because
    // focus() synchronously dispatches the focus event.
    {
        let app = app.clone();
        let closure = Closure::<dyn FnMut(Event)>::new(move |e: Event| {
            let e: MouseEvent = e.unchecked_into();
            e.prevent_default();
            let textarea = with_app(&app, |app| {
                let cell = app.cell_at(&e);
                if app.mouse_reporting(&e) {
                    let button = mouse_button(&e);
                    app.reported_button = Some(button);
                    app.last_mouse_cell = Some(cell);
                    app.report_mouse(button, MouseAction::Press, cell, &e);
                } else if e.button() == 0 {
                    let kind = match e.detail() {
                        2 => SelectionKind::Word,
                        n if n >= 3 => SelectionKind::Line,
                        _ => SelectionKind::Cell,
                    };
                    let line = app.term.viewport_line_id(cell.1);
                    app.term.selection_start((line, cell.0), kind);
                    app.selecting = true;
                }
                app.textarea.clone()
            });
            if let Some(textarea) = textarea {
                let _ = textarea.focus();
            }
        });
        canvas.add_event_listener_with_callback("mousedown", closure.as_ref().unchecked_ref())?;
        closure.forget();
    }

    listen::<MouseEvent>(&window, "mousemove", app, |app, e| {
        let cell = app.cell_at(&e);
        if app.selecting {
            let line = app.term.viewport_line_id(cell.1);
            app.term.selection_update((line, cell.0));
        } else if app.term.modes().mouse != MouseMode::Off && app.last_mouse_cell != Some(cell) {
            // Only motion over the terminal itself is reported when no button is held.
            let canvas: &web_sys::EventTarget = app.canvas.as_ref();
            let over_canvas = e.target().is_some_and(|t| t == *canvas);
            if app.reported_button.is_some() || over_canvas {
                app.last_mouse_cell = Some(cell);
                let button = app.reported_button.unwrap_or(MouseButton::None);
                app.report_mouse(button, MouseAction::Motion, cell, &e);
            }
        }
    })?;

    listen::<MouseEvent>(&window, "mouseup", app, |app, e| {
        if let Some(button) = app.reported_button.take() {
            let cell = app.cell_at(&e);
            app.report_mouse(button, MouseAction::Release, cell, &e);
        }
        if app.selecting {
            app.selecting = false;
            if app.term.selection_range().is_none() {
                app.term.selection_clear();
            }
        }
    })?;

    {
        let app = app.clone();
        let closure = Closure::<dyn FnMut(Event)>::new(move |e: Event| {
            let e: WheelEvent = e.unchecked_into();
            e.prevent_default();
            with_app(&app, |app| {
                let cell_css = app.renderer.metrics().cell_h as f64 / app.dpr;
                let lines = match e.delta_mode() {
                    WheelEvent::DOM_DELTA_PIXEL => e.delta_y() / cell_css,
                    WheelEvent::DOM_DELTA_LINE => e.delta_y(),
                    _ => e.delta_y() * app.term.rows() as f64,
                };
                app.wheel_accum += lines;
                let steps = app.wheel_accum.trunc();
                app.wheel_accum -= steps;
                let steps = steps as isize;
                if steps == 0 {
                    return;
                }
                let count = steps.unsigned_abs().min(app.term.rows());
                if app.mouse_reporting(&e) {
                    let button = if steps < 0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
                    let cell = app.cell_at(&e);
                    for _ in 0..count {
                        app.report_mouse(button, MouseAction::Press, cell, &e);
                    }
                } else if app.term.is_alt_screen() {
                    // Full-screen apps without mouse support (less, man, ...) get arrow keys.
                    let key = if steps < 0 { Key::Up } else { Key::Down };
                    for _ in 0..count {
                        if let Some(bytes) = encode_key(key, Mods::default(), app.term.modes()) {
                            app.send_input(bytes);
                        }
                    }
                } else {
                    app.term.scroll_viewport(-steps);
                }
            });
        });
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_passive(false);
        canvas.add_event_listener_with_callback_and_add_event_listener_options("wheel", closure.as_ref().unchecked_ref(), &opts)?;
        closure.forget();
    }

    listen::<MouseEvent>(&canvas, "contextmenu", app, |app, e| {
        if app.mouse_reporting(&e) {
            e.prevent_default();
        }
    })
}
