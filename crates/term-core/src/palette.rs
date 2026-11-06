//! Color theme. Shared by the renderer and by OSC color queries.

use crate::cell::Color;

pub type Rgb = [u8; 3];

#[derive(Clone, Debug)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    pub ansi: [Rgb; 16],
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            foreground: [0xff, 0xff, 0xff],
            background: [0x28, 0x2c, 0x34],
            cursor: [0xc8, 0xc8, 0xc8],
            selection: [0x4b, 0x52, 0x63],
            ansi: [
                [0x1d, 0x1f, 0x21], [0xcc, 0x66, 0x66], [0xb5, 0xbd, 0x68], [0xf0, 0xc6, 0x74],
                [0x81, 0xa2, 0xbe], [0xb2, 0x94, 0xbb], [0x8a, 0xbe, 0xb7], [0xc5, 0xc8, 0xc6],
                [0x66, 0x66, 0x66], [0xd5, 0x4e, 0x53], [0xb9, 0xca, 0x4a], [0xe7, 0xc5, 0x47],
                [0x7a, 0xa6, 0xda], [0xc3, 0x97, 0xd8], [0x70, 0xc0, 0xb1], [0xea, 0xea, 0xea],
            ],
        }
    }
}

impl Palette {
    /// The xterm 256-color palette: 16 theme colors, a 6x6x6 cube and a gray ramp.
    pub fn indexed(&self, i: u8) -> Rgb {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                [LEVELS[(i / 36) as usize], LEVELS[(i / 6 % 6) as usize], LEVELS[(i % 6) as usize]]
            }
            _ => {
                let v = 8 + 10 * (i - 232);
                [v, v, v]
            }
        }
    }

    pub fn resolve(&self, color: Color, is_fg: bool) -> Rgb {
        match color {
            Color::Default if is_fg => self.foreground,
            Color::Default => self.background,
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => [r, g, b],
        }
    }
}
