//! A VT100/xterm-compatible terminal emulator core.
//!
//! This crate has no platform dependencies: it turns a byte stream from a
//! PTY into a grid of cells, and keyboard/mouse events into byte sequences.
//! It is compiled to WebAssembly for the browser front end and can be unit
//! tested natively.

mod cell;
pub mod input;
mod palette;

use std::collections::VecDeque;

pub use cell::{Cell, Color, Flags, Row};
pub use palette::{Palette, Rgb};
use unicode_width::UnicodeWidthChar;
use vte::{Params, Parser, Perform};

const DEFAULT_SCROLLBACK: usize = 10_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    #[default]
    Off,
    /// 1000: report presses and releases.
    Click,
    /// 1002: also report motion while a button is held.
    Drag,
    /// 1003: report all motion.
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Debug)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub autowrap: bool,
    pub origin: bool,
    pub insert: bool,
    /// LNM: LF implies CR, Enter sends CRLF.
    pub newline: bool,
    pub cursor_visible: bool,
    pub cursor_blink: bool,
    pub cursor_shape: CursorShape,
    pub bracketed_paste: bool,
    pub mouse: MouseMode,
    pub mouse_sgr: bool,
    pub focus_events: bool,
    /// 2026: the application is mid-frame; the renderer should hold off.
    pub synchronized_output: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Modes {
            app_cursor: false,
            app_keypad: false,
            autowrap: true,
            origin: false,
            insert: false,
            newline: false,
            cursor_visible: true,
            cursor_blink: true,
            cursor_shape: CursorShape::Block,
            bracketed_paste: false,
            mouse: MouseMode::Off,
            mouse_sgr: false,
            focus_events: false,
            synchronized_output: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Charset {
    #[default]
    Ascii,
    DecSpecial,
}

#[derive(Clone, Copy, Debug, Default)]
struct Pen {
    fg: Color,
    bg: Color,
    flags: Flags,
}

#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    x: usize,
    y: usize,
    pen: Pen,
    /// The last column was written; the next printable character wraps first.
    pending_wrap: bool,
}

#[derive(Clone, Copy, Debug)]
struct SavedCursor {
    cursor: Cursor,
    origin: bool,
    charsets: [Charset; 2],
    active_charset: usize,
}

/// What a renderer needs to know about the cursor.
#[derive(Clone, Copy, Debug)]
pub struct CursorView {
    pub x: usize,
    /// Row within the viewport; `None` when scrolled out of view or hidden.
    pub y: Option<usize>,
    pub shape: CursorShape,
    pub blink: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Cell,
    Word,
    Line,
}

/// A point in the buffer: a stable line id (survives scrolling) and a column.
pub type Point = (u64, usize);

#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: Point,
    head: Point,
    kind: SelectionKind,
}

/// A normalized selection, inclusive on both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: Point,
    pub end: Point,
}

impl SelectionRange {
    pub fn contains(&self, line: u64, col: usize) -> bool {
        (line, col) >= self.start && (line, col) <= self.end
    }
}

pub struct Terminal {
    parser: Parser,
    st: State,
}

struct State {
    cols: usize,
    rows: usize,
    primary: Vec<Row>,
    alt: Vec<Row>,
    alt_active: bool,
    scrollback: VecDeque<Row>,
    scrollback_limit: usize,
    /// Number of lines evicted from the front of the scrollback, ever.
    /// Line ids are `lines_dropped + index into (scrollback ++ screen)`.
    lines_dropped: u64,
    /// How many lines the viewport is scrolled back into history.
    display_offset: usize,

    cursor: Cursor,
    saved: Option<SavedCursor>,
    saved_alt: Option<SavedCursor>,
    scroll_top: usize,
    scroll_bottom: usize,
    tabs: Vec<bool>,
    charsets: [Charset; 2],
    active_charset: usize,
    last_char: Option<char>,
    modes: Modes,

    selection: Option<Selection>,
    palette: Palette,
    title: String,
    title_changed: bool,
    bell: bool,
    dirty: bool,
    responses: Vec<u8>,
    clipboard: Option<String>,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        Terminal { parser: Parser::new(), st: State::new(cols.max(1), rows.max(1), Palette::default()) }
    }

    pub fn with_palette(mut self, palette: Palette) -> Self {
        self.st.palette = palette;
        self
    }

    /// Processes output from the application.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.st, bytes);
        self.st.dirty = true;
    }

    pub fn cols(&self) -> usize {
        self.st.cols
    }

    pub fn rows(&self) -> usize {
        self.st.rows
    }

    pub fn modes(&self) -> &Modes {
        &self.st.modes
    }

    pub fn palette(&self) -> &Palette {
        &self.st.palette
    }

    pub fn is_alt_screen(&self) -> bool {
        self.st.alt_active
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.st.resize(cols.max(1), rows.max(1));
    }

    /// Bytes the terminal itself must send back to the application
    /// (replies to status queries such as cursor position reports).
    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.st.responses)
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.st.dirty)
    }

    pub fn mark_dirty(&mut self) {
        self.st.dirty = true;
    }

    pub fn take_title(&mut self) -> Option<String> {
        std::mem::take(&mut self.st.title_changed).then(|| self.st.title.clone())
    }

    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.st.bell)
    }

    /// Text the application asked to place on the clipboard (OSC 52).
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.st.clipboard.take()
    }

    pub fn cursor(&self) -> CursorView {
        let st = &self.st;
        let y = st.cursor.y + st.display_offset;
        CursorView {
            x: st.cursor.x,
            y: (st.modes.cursor_visible && y < st.rows).then_some(y),
            shape: st.modes.cursor_shape,
            blink: st.modes.cursor_blink,
        }
    }

    // ---- viewport -------------------------------------------------------

    pub fn display_offset(&self) -> usize {
        self.st.display_offset
    }

    /// Scrolls the viewport; positive values move back into history.
    pub fn scroll_viewport(&mut self, delta: isize) {
        let max = if self.st.alt_active { 0 } else { self.st.scrollback.len() };
        let new = (self.st.display_offset as isize + delta).clamp(0, max as isize) as usize;
        if new != self.st.display_offset {
            self.st.display_offset = new;
            self.st.dirty = true;
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_viewport(-(self.st.display_offset as isize));
    }

    /// The row shown at viewport position `y`. May be shorter or longer than
    /// `cols()` (scrollback rows are trimmed and not reflowed).
    pub fn viewport_row(&self, y: usize) -> &Row {
        self.st.row_by_id(self.viewport_line_id(y)).expect("viewport row in range")
    }

    pub fn viewport_line_id(&self, y: usize) -> u64 {
        let st = &self.st;
        st.first_screen_id() - st.display_offset as u64 + y as u64
    }

    // ---- selection ------------------------------------------------------

    pub fn selection_start(&mut self, point: Point, kind: SelectionKind) {
        self.st.selection = Some(Selection { anchor: point, head: point, kind });
        self.st.dirty = true;
    }

    pub fn selection_update(&mut self, point: Point) {
        if let Some(sel) = &mut self.st.selection {
            sel.head = point;
            self.st.dirty = true;
        }
    }

    pub fn selection_clear(&mut self) {
        if self.st.selection.take().is_some() {
            self.st.dirty = true;
        }
    }

    pub fn selection_range(&self) -> Option<SelectionRange> {
        let st = &self.st;
        let sel = st.selection?;
        let (mut start, mut end) = if sel.anchor <= sel.head { (sel.anchor, sel.head) } else { (sel.head, sel.anchor) };
        match sel.kind {
            SelectionKind::Cell => {
                if start == end {
                    return None;
                }
            }
            SelectionKind::Word => {
                start.1 = st.word_bound(start, false);
                end.1 = st.word_bound(end, true);
            }
            SelectionKind::Line => {
                start.1 = 0;
                end.1 = st.cols.saturating_sub(1);
            }
        }
        Some(SelectionRange { start, end })
    }

    pub fn selection_text(&self) -> Option<String> {
        let range = self.selection_range()?;
        let mut out = String::new();
        for line in range.start.0..=range.end.0 {
            let Some(row) = self.st.row_by_id(line) else { continue };
            let from = if line == range.start.0 { range.start.1 } else { 0 };
            let to = if line == range.end.0 { range.end.1 + 1 } else { usize::MAX };
            out.push_str(&row.text(from, to));
            if line != range.end.0 && !row.wrapped {
                out.push('\n');
            }
        }
        Some(out)
    }
}

fn is_word_char(c: char) -> bool {
    !c.is_whitespace() && !"'\"`|│:;,()[]{}<>".contains(c)
}

impl State {
    fn new(cols: usize, rows: usize, palette: Palette) -> Self {
        State {
            cols,
            rows,
            primary: vec![Row::new(cols, Color::Default); rows],
            alt: vec![Row::new(cols, Color::Default); rows],
            alt_active: false,
            scrollback: VecDeque::new(),
            scrollback_limit: DEFAULT_SCROLLBACK,
            lines_dropped: 0,
            display_offset: 0,
            cursor: Cursor::default(),
            saved: None,
            saved_alt: None,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            tabs: default_tabs(cols),
            charsets: [Charset::Ascii; 2],
            active_charset: 0,
            last_char: None,
            modes: Modes::default(),
            selection: None,
            palette,
            title: String::new(),
            title_changed: false,
            bell: false,
            dirty: true,
            responses: Vec::new(),
            clipboard: None,
        }
    }

    // ---- buffer access --------------------------------------------------

    fn screen(&self) -> &Vec<Row> {
        if self.alt_active { &self.alt } else { &self.primary }
    }

    fn screen_mut(&mut self) -> &mut Vec<Row> {
        if self.alt_active { &mut self.alt } else { &mut self.primary }
    }

    fn row_mut(&mut self, y: usize) -> &mut Row {
        &mut self.screen_mut()[y]
    }

    fn first_screen_id(&self) -> u64 {
        self.lines_dropped + self.scrollback.len() as u64
    }

    fn row_by_id(&self, id: u64) -> Option<&Row> {
        let idx = usize::try_from(id.checked_sub(self.lines_dropped)?).ok()?;
        let sb = self.scrollback.len();
        if idx < sb { self.scrollback.get(idx) } else { self.screen().get(idx - sb) }
    }

    fn word_bound(&self, (line, col): Point, forward: bool) -> usize {
        let Some(row) = self.row_by_id(line) else { return col };
        if !is_word_char(row.get(col).ch) {
            return col;
        }
        let mut c = col;
        if forward {
            while c + 1 < row.cells.len() && is_word_char(row.get(c + 1).ch) {
                c += 1;
            }
        } else {
            while c > 0 && is_word_char(row.get(c - 1).ch) {
                c -= 1;
            }
        }
        c
    }

    fn blank(&self) -> Cell {
        Cell::blank(self.cursor.pen.bg)
    }

    fn blank_row(&self) -> Row {
        Row::new(self.cols, self.cursor.pen.bg)
    }

    fn push_scrollback(&mut self, mut row: Row) {
        row.trim();
        self.scrollback.push_back(row);
        if self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
            self.lines_dropped += 1;
        }
        // Keep the viewport pinned to the same content while scrolled back.
        if self.display_offset > 0 {
            self.display_offset = (self.display_offset + 1).min(self.scrollback.len());
        }
    }

    // ---- scrolling ------------------------------------------------------

    fn scroll_up(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let n = n.min(bottom - top + 1);
        for _ in 0..n {
            let blank = self.blank_row();
            let screen = self.screen_mut();
            let row = screen.remove(top);
            screen.insert(bottom, blank);
            if top == 0 && !self.alt_active {
                self.push_scrollback(row);
            }
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let n = n.min(bottom - top + 1);
        for _ in 0..n {
            let blank = self.blank_row();
            let screen = self.screen_mut();
            screen.remove(bottom);
            screen.insert(top, blank);
        }
    }

    fn index(&mut self) {
        if self.cursor.y == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.y + 1 < self.rows {
            self.cursor.y += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.cursor.y == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor.y > 0 {
            self.cursor.y -= 1;
        }
    }

    fn linefeed(&mut self) {
        self.cursor.pending_wrap = false;
        self.index();
        if self.modes.newline {
            self.cursor.x = 0;
        }
    }

    // ---- cursor movement ------------------------------------------------

    fn goto(&mut self, x: usize, y: usize) {
        let (min_y, max_y) = if self.modes.origin { (self.scroll_top, self.scroll_bottom) } else { (0, self.rows - 1) };
        self.cursor.x = x.min(self.cols - 1);
        self.cursor.y = y.clamp(min_y, max_y);
        self.cursor.pending_wrap = false;
    }

    /// CUP-style absolute positioning, relative to the scroll region in origin mode.
    fn goto_origin(&mut self, x: usize, y: usize) {
        let base = if self.modes.origin { self.scroll_top } else { 0 };
        self.goto(x, base + y);
    }

    fn move_up(&mut self, n: usize) {
        let min = if self.cursor.y >= self.scroll_top { self.scroll_top } else { 0 };
        self.cursor.y = self.cursor.y.saturating_sub(n).max(min);
        self.cursor.pending_wrap = false;
    }

    fn move_down(&mut self, n: usize) {
        let max = if self.cursor.y <= self.scroll_bottom { self.scroll_bottom } else { self.rows - 1 };
        self.cursor.y = (self.cursor.y + n).min(max);
        self.cursor.pending_wrap = false;
    }

    fn save_cursor(&mut self) {
        let saved = SavedCursor {
            cursor: self.cursor,
            origin: self.modes.origin,
            charsets: self.charsets,
            active_charset: self.active_charset,
        };
        if self.alt_active { self.saved_alt = Some(saved) } else { self.saved = Some(saved) }
    }

    fn restore_cursor(&mut self) {
        let saved = if self.alt_active { self.saved_alt } else { self.saved };
        match saved {
            Some(s) => {
                self.cursor = s.cursor;
                self.modes.origin = s.origin;
                self.charsets = s.charsets;
                self.active_charset = s.active_charset;
            }
            None => {
                self.cursor = Cursor::default();
                self.modes.origin = false;
            }
        }
        self.cursor.x = self.cursor.x.min(self.cols - 1);
        self.cursor.y = self.cursor.y.min(self.rows - 1);
    }

    fn tab_forward(&mut self, n: usize) {
        for _ in 0..n {
            let next = (self.cursor.x + 1..self.cols).find(|&x| self.tabs[x]);
            self.cursor.x = next.unwrap_or(self.cols - 1);
        }
        self.cursor.pending_wrap = false;
    }

    fn tab_backward(&mut self, n: usize) {
        for _ in 0..n {
            let prev = (0..self.cursor.x).rev().find(|&x| self.tabs[x]);
            self.cursor.x = prev.unwrap_or(0);
        }
        self.cursor.pending_wrap = false;
    }

    // ---- printing -------------------------------------------------------

    fn print_char(&mut self, c: char) {
        let c = if self.charsets[self.active_charset] == Charset::DecSpecial { dec_special(c) } else { c };
        let width = match c.width() {
            Some(w @ 1..=2) => w,
            // Zero-width (combining) characters are not stored separately.
            _ => return,
        };
        self.last_char = Some(c);

        if self.cursor.pending_wrap && self.modes.autowrap {
            self.wrap_line();
        }
        if width == 2 && self.cursor.x + 1 >= self.cols {
            if self.modes.autowrap {
                let (x, y, blank) = (self.cursor.x, self.cursor.y, self.blank());
                self.row_mut(y).cells[x] = blank;
                self.wrap_line();
            } else {
                return;
            }
        }

        let (x, y, cols) = (self.cursor.x, self.cursor.y, self.cols);
        let pen = self.cursor.pen;
        let insert = self.modes.insert;
        let blank = self.blank();
        let row = self.row_mut(y);
        if insert {
            for _ in 0..width {
                row.cells.insert(x, blank);
            }
            row.cells.truncate(cols);
        }
        // Writing over half of a wide character destroys the other half.
        if row.cells[x].flags.contains(Flags::SPACER) && x > 0 {
            row.cells[x - 1] = blank;
        }
        let end = x + width;
        if end < cols && row.cells[end].flags.contains(Flags::SPACER) {
            row.cells[end] = blank;
        }

        let mut flags = pen.flags;
        if width == 2 {
            flags.insert(Flags::WIDE);
        }
        row.cells[x] = Cell { ch: c, fg: pen.fg, bg: pen.bg, flags };
        if width == 2 {
            row.cells[x + 1] = Cell { ch: ' ', fg: pen.fg, bg: pen.bg, flags: pen.flags | Flags::SPACER };
        }
        if insert {
            row.fix_wide();
        }

        if end >= cols {
            self.cursor.x = cols - 1;
            self.cursor.pending_wrap = self.modes.autowrap;
        } else {
            self.cursor.x = end;
        }
    }

    fn wrap_line(&mut self) {
        let y = self.cursor.y;
        self.row_mut(y).wrapped = true;
        self.cursor.x = 0;
        self.cursor.pending_wrap = false;
        self.index();
    }

    // ---- erasing --------------------------------------------------------

    fn erase_cells(&mut self, y: usize, from: usize, to: usize) {
        let blank = self.blank();
        let row = self.row_mut(y);
        let to = to.min(row.cells.len());
        for cell in row.cells.get_mut(from..to).unwrap_or(&mut []) {
            *cell = blank;
        }
        row.fix_wide();
    }

    fn erase_display(&mut self, mode: u16) {
        let (x, y, rows, cols) = (self.cursor.x, self.cursor.y, self.rows, self.cols);
        match mode {
            0 => {
                self.erase_cells(y, x, cols);
                self.row_mut(y).wrapped = false;
                for r in y + 1..rows {
                    *self.row_mut(r) = self.blank_row();
                }
            }
            1 => {
                for r in 0..y {
                    *self.row_mut(r) = self.blank_row();
                }
                self.erase_cells(y, 0, x + 1);
            }
            2 => {
                for r in 0..rows {
                    *self.row_mut(r) = self.blank_row();
                }
            }
            3 => {
                self.lines_dropped += self.scrollback.len() as u64;
                self.scrollback.clear();
                self.display_offset = 0;
                self.selection = None;
            }
            _ => {}
        }
        self.cursor.pending_wrap = false;
    }

    fn erase_line(&mut self, mode: u16) {
        let (x, y, cols) = (self.cursor.x, self.cursor.y, self.cols);
        match mode {
            0 => {
                self.erase_cells(y, x, cols);
                self.row_mut(y).wrapped = false;
            }
            1 => self.erase_cells(y, 0, x + 1),
            2 => *self.row_mut(y) = self.blank_row(),
            _ => {}
        }
        self.cursor.pending_wrap = false;
    }

    fn insert_lines(&mut self, n: usize) {
        let y = self.cursor.y;
        if y < self.scroll_top || y > self.scroll_bottom {
            return;
        }
        let bottom = self.scroll_bottom;
        for _ in 0..n.min(bottom - y + 1) {
            let blank = self.blank_row();
            let screen = self.screen_mut();
            screen.remove(bottom);
            screen.insert(y, blank);
        }
        self.cursor.x = 0;
        self.cursor.pending_wrap = false;
    }

    fn delete_lines(&mut self, n: usize) {
        let y = self.cursor.y;
        if y < self.scroll_top || y > self.scroll_bottom {
            return;
        }
        let bottom = self.scroll_bottom;
        for _ in 0..n.min(bottom - y + 1) {
            let blank = self.blank_row();
            let screen = self.screen_mut();
            screen.remove(y);
            screen.insert(bottom, blank);
        }
        self.cursor.x = 0;
        self.cursor.pending_wrap = false;
    }

    fn insert_chars(&mut self, n: usize) {
        let (x, y, cols, blank) = (self.cursor.x, self.cursor.y, self.cols, self.blank());
        let row = self.row_mut(y);
        for _ in 0..n.min(cols - x) {
            row.cells.insert(x, blank);
        }
        row.cells.truncate(cols);
        row.fix_wide();
        self.cursor.pending_wrap = false;
    }

    fn delete_chars(&mut self, n: usize) {
        let (x, y, cols, blank) = (self.cursor.x, self.cursor.y, self.cols, self.blank());
        let row = self.row_mut(y);
        let n = n.min(cols - x);
        row.cells.drain(x..x + n);
        row.cells.resize(cols, blank);
        row.fix_wide();
        self.cursor.pending_wrap = false;
    }

    // ---- screens & modes --------------------------------------------------

    fn set_alt_screen(&mut self, enable: bool, clear: bool) {
        if enable == self.alt_active {
            return;
        }
        self.alt_active = enable;
        if enable && clear {
            let blank = self.blank_row();
            self.alt.iter_mut().for_each(|r| *r = blank.clone());
        }
        self.display_offset = 0;
        self.selection = None;
    }

    fn set_private_mode(&mut self, mode: u16, on: bool) {
        match mode {
            1 => self.modes.app_cursor = on,
            6 => {
                self.modes.origin = on;
                self.goto_origin(0, 0);
            }
            7 => self.modes.autowrap = on,
            12 => self.modes.cursor_blink = on,
            25 => self.modes.cursor_visible = on,
            9 | 1000 => self.modes.mouse = if on { MouseMode::Click } else { MouseMode::Off },
            1002 => self.modes.mouse = if on { MouseMode::Drag } else { MouseMode::Off },
            1003 => self.modes.mouse = if on { MouseMode::Motion } else { MouseMode::Off },
            1004 => self.modes.focus_events = on,
            1006 => self.modes.mouse_sgr = on,
            47 | 1047 => self.set_alt_screen(on, true),
            1048 => {
                if on { self.save_cursor() } else { self.restore_cursor() }
            }
            1049 => {
                if on {
                    self.save_cursor();
                    self.set_alt_screen(true, true);
                } else {
                    self.set_alt_screen(false, false);
                    self.restore_cursor();
                }
            }
            2004 => self.modes.bracketed_paste = on,
            2026 => self.modes.synchronized_output = on,
            _ => {}
        }
    }

    fn private_mode_state(&self, mode: u16) -> Option<bool> {
        let m = &self.modes;
        Some(match mode {
            1 => m.app_cursor,
            6 => m.origin,
            7 => m.autowrap,
            12 => m.cursor_blink,
            25 => m.cursor_visible,
            1000 => m.mouse == MouseMode::Click,
            1002 => m.mouse == MouseMode::Drag,
            1003 => m.mouse == MouseMode::Motion,
            1004 => m.focus_events,
            1006 => m.mouse_sgr,
            47 | 1047 | 1049 => self.alt_active,
            2004 => m.bracketed_paste,
            2026 => m.synchronized_output,
            _ => return None,
        })
    }

    fn soft_reset(&mut self) {
        let shape = self.modes.cursor_shape;
        self.modes = Modes { cursor_shape: shape, ..Modes::default() };
        self.cursor.pen = Pen::default();
        self.cursor.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.charsets = [Charset::Ascii; 2];
        self.active_charset = 0;
        self.saved = None;
        self.saved_alt = None;
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        let old_rows = self.rows;

        // On the primary screen, keep the cursor line visible when shrinking
        // by pushing lines into history, and pull them back when growing.
        if !self.alt_active && rows < old_rows {
            let push = (self.cursor.y + 1).saturating_sub(rows);
            for _ in 0..push {
                let row = self.primary.remove(0);
                self.push_scrollback(row);
            }
            self.cursor.y -= push;
        } else if !self.alt_active && rows > old_rows {
            let pull = (rows - old_rows).min(self.scrollback.len());
            for _ in 0..pull {
                let row = self.scrollback.pop_back().expect("scrollback row");
                self.primary.insert(0, row);
            }
            self.cursor.y += pull;
        }
        for screen in [&mut self.primary, &mut self.alt] {
            screen.resize_with(rows, || Row::new(cols, Color::Default));
            screen.iter_mut().for_each(|r| r.resize(cols));
        }

        self.cols = cols;
        self.rows = rows;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.tabs = default_tabs(cols);
        self.cursor.x = self.cursor.x.min(cols - 1);
        self.cursor.y = self.cursor.y.min(rows - 1);
        self.cursor.pending_wrap = false;
        self.display_offset = self.display_offset.min(self.scrollback.len());
        self.selection = None;
        self.dirty = true;
    }

    // ---- control sequences ------------------------------------------------

    fn sgr(&mut self, params: &Params) {
        let groups: Vec<&[u16]> = params.iter().collect();
        let pen = &mut self.cursor.pen;
        if groups.is_empty() {
            *pen = Pen::default();
            return;
        }
        let mut i = 0;
        while i < groups.len() {
            let g = groups[i];
            match g[0] {
                0 => *pen = Pen::default(),
                1 => pen.flags.insert(Flags::BOLD),
                2 => pen.flags.insert(Flags::DIM),
                3 => pen.flags.insert(Flags::ITALIC),
                4 => {
                    pen.flags.remove(Flags::ANY_UNDERLINE);
                    match g.get(1) {
                        Some(0) => {}
                        Some(2) => pen.flags.insert(Flags::DOUBLE_UNDERLINE),
                        Some(3) => pen.flags.insert(Flags::CURLY_UNDERLINE),
                        _ => pen.flags.insert(Flags::UNDERLINE),
                    }
                }
                5 | 6 => pen.flags.insert(Flags::BLINK),
                7 => pen.flags.insert(Flags::INVERSE),
                8 => pen.flags.insert(Flags::HIDDEN),
                9 => pen.flags.insert(Flags::STRIKE),
                21 => {
                    pen.flags.remove(Flags::ANY_UNDERLINE);
                    pen.flags.insert(Flags::DOUBLE_UNDERLINE);
                }
                22 => pen.flags.remove(Flags::BOLD | Flags::DIM),
                23 => pen.flags.remove(Flags::ITALIC),
                24 => pen.flags.remove(Flags::ANY_UNDERLINE),
                25 => pen.flags.remove(Flags::BLINK),
                27 => pen.flags.remove(Flags::INVERSE),
                28 => pen.flags.remove(Flags::HIDDEN),
                29 => pen.flags.remove(Flags::STRIKE),
                n @ 30..=37 => pen.fg = Color::Indexed((n - 30) as u8),
                39 => pen.fg = Color::Default,
                n @ 40..=47 => pen.bg = Color::Indexed((n - 40) as u8),
                49 => pen.bg = Color::Default,
                n @ 90..=97 => pen.fg = Color::Indexed((n - 90 + 8) as u8),
                n @ 100..=107 => pen.bg = Color::Indexed((n - 100 + 8) as u8),
                n @ (38 | 48 | 58) => {
                    let (color, consumed) = extended_color(&groups, i);
                    i += consumed;
                    match (n, color) {
                        (38, Some(c)) => pen.fg = c,
                        (48, Some(c)) => pen.bg = c,
                        _ => {} // underline color is not rendered
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn reply(&mut self, s: &str) {
        self.responses.extend_from_slice(s.as_bytes());
    }

    fn osc_color_reply(&mut self, prefix: &str, rgb: Rgb, bell: bool) {
        let [r, g, b] = rgb;
        let st = if bell { "\x07" } else { "\x1b\\" };
        self.reply(&format!("\x1b]{prefix};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}{st}"));
    }
}

/// Parses `38;5;n`, `38;2;r;g;b` and their colon-separated forms.
/// Returns the color and the number of extra parameter groups consumed.
fn extended_color(groups: &[&[u16]], i: usize) -> (Option<Color>, usize) {
    let g = groups[i];
    if g.len() > 1 {
        let color = match g[1] {
            5 => g.get(2).map(|&n| Color::Indexed(n as u8)),
            // 38:2:<colorspace>:r:g:b, or 38:2:r:g:b without colorspace.
            2 if g.len() >= 6 => Some(Color::Rgb(g[3] as u8, g[4] as u8, g[5] as u8)),
            2 if g.len() == 5 => Some(Color::Rgb(g[2] as u8, g[3] as u8, g[4] as u8)),
            _ => None,
        };
        return (color, 0);
    }
    let arg = |k: usize| groups.get(i + k).map(|g| g[0]);
    match arg(1) {
        Some(5) => (arg(2).map(|n| Color::Indexed(n as u8)), 2),
        Some(2) => match (arg(2), arg(3), arg(4)) {
            (Some(r), Some(g), Some(b)) => (Some(Color::Rgb(r as u8, g as u8, b as u8)), 4),
            _ => (None, groups.len() - i - 1),
        },
        _ => (None, 0),
    }
}

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|x| x % 8 == 0 && x != 0).collect()
}

/// The DEC Special Graphics character set, used for line drawing.
fn dec_special(c: char) -> char {
    match c {
        '_' => ' ',
        '`' => '◆',
        'a' => '▒',
        'b' => '␉',
        'c' => '␌',
        'd' => '␍',
        'e' => '␊',
        'f' => '°',
        'g' => '±',
        'h' => '␤',
        'i' => '␋',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        c => c,
    }
}

fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in input.iter().filter(|&&c| c != b'=' && !c.is_ascii_whitespace()) {
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

impl Perform for State {
    fn print(&mut self, c: char) {
        self.print_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.bell = true,
            0x08 => {
                self.cursor.x = self.cursor.x.saturating_sub(1);
                self.cursor.pending_wrap = false;
            }
            0x09 => self.tab_forward(1),
            0x0a..=0x0c => self.linefeed(),
            0x0d => {
                self.cursor.x = 0;
                self.cursor.pending_wrap = false;
            }
            0x0e => self.active_charset = 1,
            0x0f => self.active_charset = 0,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // First value of the n-th parameter; 0 means "default" for most sequences.
        let raw = |n: usize| params.iter().nth(n).map_or(0, |p| p[0]);
        let arg = |n: usize, default: u16| match raw(n) {
            0 => default,
            v => v,
        } as usize;

        match (intermediates, action) {
            ([], '@') => self.insert_chars(arg(0, 1)),
            ([], 'A') => self.move_up(arg(0, 1)),
            ([], 'B' | 'e') => self.move_down(arg(0, 1)),
            ([], 'C' | 'a') => {
                self.cursor.x = (self.cursor.x + arg(0, 1)).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            ([], 'D') => {
                self.cursor.x = self.cursor.x.saturating_sub(arg(0, 1));
                self.cursor.pending_wrap = false;
            }
            ([], 'E') => {
                self.move_down(arg(0, 1));
                self.cursor.x = 0;
            }
            ([], 'F') => {
                self.move_up(arg(0, 1));
                self.cursor.x = 0;
            }
            ([], 'G' | '`') => {
                self.cursor.x = (arg(0, 1) - 1).min(self.cols - 1);
                self.cursor.pending_wrap = false;
            }
            ([], 'H' | 'f') => self.goto_origin(arg(1, 1) - 1, arg(0, 1) - 1),
            ([], 'I') => self.tab_forward(arg(0, 1)),
            ([], 'J') => self.erase_display(raw(0)),
            ([b'?'], 'J') => self.erase_display(raw(0)),
            ([], 'K') | ([b'?'], 'K') => self.erase_line(raw(0)),
            ([], 'L') => self.insert_lines(arg(0, 1)),
            ([], 'M') => self.delete_lines(arg(0, 1)),
            ([], 'P') => self.delete_chars(arg(0, 1)),
            ([], 'S') => self.scroll_up(arg(0, 1)),
            ([], 'T') => self.scroll_down(arg(0, 1)),
            ([], 'X') => {
                let (x, y) = (self.cursor.x, self.cursor.y);
                self.erase_cells(y, x, x + arg(0, 1));
                self.cursor.pending_wrap = false;
            }
            ([], 'Z') => self.tab_backward(arg(0, 1)),
            ([], 'b') => {
                if let Some(c) = self.last_char {
                    for _ in 0..arg(0, 1).min(65535) {
                        self.print_char(c);
                    }
                }
            }
            ([], 'c') if raw(0) == 0 => self.reply("\x1b[?62;22c"),
            ([b'>'], 'c') if raw(0) == 0 => self.reply("\x1b[>1;10;0c"),
            ([], 'd') => {
                let x = self.cursor.x;
                self.goto_origin(x, arg(0, 1) - 1);
            }
            ([], 'g') => match raw(0) {
                0 => {
                    let x = self.cursor.x;
                    self.tabs[x] = false;
                }
                3 => self.tabs.iter_mut().for_each(|t| *t = false),
                _ => {}
            },
            ([], 'h' | 'l') => {
                let on = action == 'h';
                for p in params.iter() {
                    match p[0] {
                        4 => self.modes.insert = on,
                        20 => self.modes.newline = on,
                        _ => {}
                    }
                }
            }
            ([b'?'], 'h' | 'l') => {
                for p in params.iter() {
                    self.set_private_mode(p[0], action == 'h');
                }
            }
            ([], 'm') => self.sgr(params),
            ([], 'n') => match raw(0) {
                5 => self.reply("\x1b[0n"),
                6 => {
                    let y = self.cursor.y - if self.modes.origin { self.scroll_top } else { 0 };
                    let x = self.cursor.x;
                    self.reply(&format!("\x1b[{};{}R", y + 1, x + 1));
                }
                _ => {}
            },
            ([b'?', b'$'], 'p') => {
                let mode = raw(0);
                let state = match self.private_mode_state(mode) {
                    Some(true) => 1,
                    Some(false) => 2,
                    None => 0,
                };
                self.reply(&format!("\x1b[?{mode};{state}$y"));
            }
            ([b'!'], 'p') => self.soft_reset(),
            ([b' '], 'q') => {
                let (shape, blink) = match raw(0) {
                    0 | 1 => (CursorShape::Block, true),
                    2 => (CursorShape::Block, false),
                    3 => (CursorShape::Underline, true),
                    4 => (CursorShape::Underline, false),
                    5 => (CursorShape::Bar, true),
                    _ => (CursorShape::Bar, false),
                };
                self.modes.cursor_shape = shape;
                self.modes.cursor_blink = blink;
            }
            ([b'>'], 'q') => self.reply(concat!("\x1bP>|webterm ", env!("CARGO_PKG_VERSION"), "\x1b\\")),
            ([], 'r') => {
                let top = arg(0, 1) - 1;
                let bottom = arg(1, self.rows as u16).min(self.rows) - 1;
                if top < bottom {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    self.goto_origin(0, 0);
                }
            }
            ([], 's') => self.save_cursor(),
            ([], 'u') => self.restore_cursor(),
            ([], 't') if raw(0) == 18 => {
                let (rows, cols) = (self.rows, self.cols);
                self.reply(&format!("\x1b[8;{rows};{cols}t"));
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => {
                self.cursor.pending_wrap = false;
                self.index();
            }
            ([], b'E') => {
                self.cursor.x = 0;
                self.cursor.pending_wrap = false;
                self.index();
            }
            ([], b'H') => {
                let x = self.cursor.x;
                self.tabs[x] = true;
            }
            ([], b'M') => {
                self.cursor.pending_wrap = false;
                self.reverse_index();
            }
            ([], b'c') => {
                let palette = std::mem::take(&mut self.palette);
                *self = State::new(self.cols, self.rows, palette);
            }
            ([], b'=') => self.modes.app_keypad = true,
            ([], b'>') => self.modes.app_keypad = false,
            ([b'('], c) => self.charsets[0] = if c == b'0' { Charset::DecSpecial } else { Charset::Ascii },
            ([b')'], c) => self.charsets[1] = if c == b'0' { Charset::DecSpecial } else { Charset::Ascii },
            ([b'#'], b'8') => {
                for y in 0..self.rows {
                    let cols = self.cols;
                    self.row_mut(y).cells = vec![Cell { ch: 'E', ..Cell::default() }; cols];
                }
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let Some(cmd) = params.first().and_then(|p| std::str::from_utf8(p).ok()) else { return };
        match cmd {
            "0" | "2" => {
                if params.len() > 1 {
                    // The title may itself contain ';', which the parser split on.
                    let joined = params[1..].join(&b';');
                    self.title = String::from_utf8_lossy(&joined).into_owned();
                    self.title_changed = true;
                }
            }
            "4" => {
                // OSC 4 ; index ; ? — palette color query.
                for pair in params[1..].chunks(2) {
                    if let [idx, b"?"] = pair
                        && let Some(i) = std::str::from_utf8(idx).ok().and_then(|s| s.parse::<u8>().ok())
                    {
                        let rgb = self.palette.indexed(i);
                        self.osc_color_reply(&format!("4;{i}"), rgb, bell_terminated);
                    }
                }
            }
            "10" | "11" | "12" if params.get(1) == Some(&&b"?"[..]) => {
                let rgb = match cmd {
                    "10" => self.palette.foreground,
                    "11" => self.palette.background,
                    _ => self.palette.cursor,
                };
                self.osc_color_reply(cmd, rgb, bell_terminated);
            }
            "52" => {
                if let Some(data) = params.get(2)
                    && *data != b"?"
                    && let Some(bytes) = base64_decode(data)
                {
                    self.clipboard = Some(String::from_utf8_lossy(&bytes).into_owned());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_text(t: &Terminal) -> Vec<String> {
        (0..t.rows()).map(|y| t.viewport_row(y).text(0, usize::MAX)).collect()
    }

    #[test]
    fn prints_and_wraps() {
        let mut t = Terminal::new(5, 3);
        t.feed(b"hello world");
        assert_eq!(screen_text(&t), ["hello", " worl", "d"]);
        assert!(t.viewport_row(0).wrapped);
    }

    #[test]
    fn pending_wrap_and_cr() {
        let mut t = Terminal::new(5, 2);
        t.feed(b"abcde\r\nx");
        assert_eq!(screen_text(&t), ["abcde", "x"]);
    }

    #[test]
    fn scrolls_into_scrollback() {
        let mut t = Terminal::new(10, 2);
        t.feed(b"1\r\n2\r\n3\r\n4");
        assert_eq!(screen_text(&t), ["3", "4"]);
        t.scroll_viewport(2);
        assert_eq!(screen_text(&t), ["1", "2"]);
        t.scroll_viewport(-100);
        assert_eq!(screen_text(&t), ["3", "4"]);
    }

    #[test]
    fn cursor_movement_and_erase() {
        let mut t = Terminal::new(10, 3);
        t.feed(b"aaaaaaaaaa\x1b[2;3Hb\x1b[1;5H\x1b[K");
        assert_eq!(screen_text(&t), ["aaaa", "  b", ""]);
        t.feed(b"\x1b[2J");
        assert_eq!(screen_text(&t), ["", "", ""]);
    }

    #[test]
    fn sgr_colors() {
        let mut t = Terminal::new(10, 1);
        t.feed(b"\x1b[1;31mA\x1b[38;2;1;2;3mB\x1b[48:5:200mC\x1b[0mD");
        let row = t.viewport_row(0);
        assert_eq!(row.cells[0].fg, Color::Indexed(1));
        assert!(row.cells[0].flags.contains(Flags::BOLD));
        assert_eq!(row.cells[1].fg, Color::Rgb(1, 2, 3));
        assert_eq!(row.cells[2].bg, Color::Indexed(200));
        assert_eq!(row.cells[3], Cell { ch: 'D', ..Cell::default() });
    }

    #[test]
    fn wide_chars() {
        let mut t = Terminal::new(4, 2);
        t.feed("a中b中".as_bytes());
        let row = t.viewport_row(0);
        assert!(row.cells[1].flags.contains(Flags::WIDE));
        assert!(row.cells[2].flags.contains(Flags::SPACER));
        // The second wide char doesn't fit in the last column and wraps.
        assert_eq!(screen_text(&t), ["a中b", "中"]);
    }

    #[test]
    fn scroll_region() {
        let mut t = Terminal::new(5, 4);
        t.feed(b"1\r\n2\r\n3\r\n4\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(screen_text(&t), ["1", "3", "", "4"]);
    }

    #[test]
    fn alt_screen() {
        let mut t = Terminal::new(5, 2);
        t.feed(b"main\x1b[?1049h\x1b[Halt");
        assert_eq!(screen_text(&t), ["alt", ""]);
        t.feed(b"\x1b[?1049l");
        assert_eq!(screen_text(&t), ["main", ""]);
        assert_eq!(t.cursor().x, 4);
    }

    #[test]
    fn cursor_position_report() {
        let mut t = Terminal::new(10, 5);
        t.feed(b"\x1b[3;4H\x1b[6n");
        assert_eq!(t.take_responses(), b"\x1b[3;4R");
    }

    #[test]
    fn dec_line_drawing() {
        let mut t = Terminal::new(5, 1);
        t.feed(b"\x1b(0lqk\x1b(Bq");
        assert_eq!(screen_text(&t), ["┌─┐q"]);
    }

    #[test]
    fn selection_text_joins_wrapped_lines() {
        let mut t = Terminal::new(5, 4);
        t.feed(b"hello world\r\nnext");
        let id = t.viewport_line_id(0);
        t.selection_start((id, 0), SelectionKind::Cell);
        t.selection_update((id + 3, 3));
        assert_eq!(t.selection_text().unwrap(), "hello world\nnext");
        t.selection_start((id, 1), SelectionKind::Word);
        assert_eq!(t.selection_text().unwrap(), "hello");
    }

    #[test]
    fn resize_keeps_cursor_visible() {
        let mut t = Terminal::new(10, 4);
        t.feed(b"1\r\n2\r\n3\r\n4");
        t.resize(10, 2);
        assert_eq!(screen_text(&t), ["3", "4"]);
        t.resize(10, 4);
        assert_eq!(screen_text(&t), ["1", "2", "3", "4"]);
    }

    #[test]
    fn osc_title_and_clipboard() {
        let mut t = Terminal::new(10, 1);
        t.feed(b"\x1b]0;my;title\x07\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(t.take_title().as_deref(), Some("my;title"));
        assert_eq!(t.take_clipboard().as_deref(), Some("hello"));
    }
}
