//! Screen cells and rows.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Flags(u16);

impl Flags {
    pub const BOLD: Flags = Flags(1 << 0);
    pub const DIM: Flags = Flags(1 << 1);
    pub const ITALIC: Flags = Flags(1 << 2);
    pub const UNDERLINE: Flags = Flags(1 << 3);
    pub const DOUBLE_UNDERLINE: Flags = Flags(1 << 4);
    pub const CURLY_UNDERLINE: Flags = Flags(1 << 5);
    pub const BLINK: Flags = Flags(1 << 6);
    pub const INVERSE: Flags = Flags(1 << 7);
    pub const HIDDEN: Flags = Flags(1 << 8);
    pub const STRIKE: Flags = Flags(1 << 9);
    /// First half of a double-width character.
    pub const WIDE: Flags = Flags(1 << 10);
    /// Second half of a double-width character (renders nothing).
    pub const SPACER: Flags = Flags(1 << 11);

    pub const ANY_UNDERLINE: Flags = Flags(Self::UNDERLINE.0 | Self::DOUBLE_UNDERLINE.0 | Self::CURLY_UNDERLINE.0);

    pub const fn empty() -> Self {
        Flags(0)
    }

    pub fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Flags) -> bool {
        self.0 & other.0 != 0
    }

    pub fn insert(&mut self, other: Flags) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Flags) {
        self.0 &= !other.0;
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Flags {
    type Output = Flags;
    fn bitor(self, rhs: Flags) -> Flags {
        Flags(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: Color::Default, bg: Color::Default, flags: Flags::empty() }
    }
}

impl Cell {
    /// An erased cell. Erase operations keep the current background color (BCE).
    pub fn blank(bg: Color) -> Self {
        Cell { bg, ..Cell::default() }
    }

    fn is_default_blank(&self) -> bool {
        *self == Cell::default()
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// The line was soft-wrapped into the next row (used for copy/selection).
    pub wrapped: bool,
}

impl Row {
    pub fn new(cols: usize, bg: Color) -> Self {
        Row { cells: vec![Cell::blank(bg); cols], wrapped: false }
    }

    pub fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::default());
        if let Some(last) = self.cells.last_mut()
            && last.flags.contains(Flags::WIDE)
        {
            *last = Cell::blank(last.bg);
        }
    }

    /// Drops trailing default cells; rows in scrollback are padded at render time.
    pub fn trim(&mut self) {
        let len = self.cells.iter().rposition(|c| !c.is_default_blank()).map_or(0, |i| i + 1);
        self.cells.truncate(len);
        self.cells.shrink_to_fit();
    }

    pub fn get(&self, x: usize) -> Cell {
        self.cells.get(x).copied().unwrap_or_default()
    }

    /// Repairs wide characters that lost their other half after an edit.
    pub fn fix_wide(&mut self) {
        let len = self.cells.len();
        for x in 0..len {
            let flags = self.cells[x].flags;
            if flags.contains(Flags::WIDE) && !(x + 1 < len && self.cells[x + 1].flags.contains(Flags::SPACER)) {
                self.cells[x] = Cell::blank(self.cells[x].bg);
            } else if flags.contains(Flags::SPACER) && !(x > 0 && self.cells[x - 1].flags.contains(Flags::WIDE)) {
                self.cells[x] = Cell::blank(self.cells[x].bg);
            }
        }
    }

    /// Text of cells `[from, to)`. Trailing whitespace is dropped unless the
    /// line continues on the next row.
    pub fn text(&self, from: usize, to: usize) -> String {
        let to = to.min(self.cells.len());
        let mut s: String = self.cells.get(from..to).unwrap_or(&[]).iter()
            .filter(|c| !c.flags.contains(Flags::SPACER))
            .map(|c| c.ch)
            .collect();
        if !self.wrapped {
            s.truncate(s.trim_end().len());
        }
        s
    }
}
