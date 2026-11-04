//! Text rasterization for GDI (TextOutA/ExtTextOutA/DrawTextA) with fontdue and the bundled Liberation Sans
//! (SIL OFL 1.1, `assets/fonts/LICENSE-LiberationSans.txt`). Every face name (the game asks for
//! "MS Sans Serif" from the registry `Font` value) maps to Liberation Sans; weight >= 600 uses the bold face.
//!
//! Sizes follow GDI: `lfHeight < 0` = character (em) height in pixels, `lfHeight > 0` = cell height
//! (ascent + descent), 0 = default (13 px cell, like MS Sans Serif 8pt at 96 dpi). Advances are rounded to whole
//! pixels per glyph, like GDI.

use std::collections::HashMap;
use std::sync::OnceLock;

static REGULAR_TTF: &[u8] = include_bytes!("../assets/fonts/LiberationSans-Regular.ttf");
static BOLD_TTF: &[u8] = include_bytes!("../assets/fonts/LiberationSans-Bold.ttf");

fn faces() -> &'static [fontdue::Font; 2] {
    static F: OnceLock<[fontdue::Font; 2]> = OnceLock::new();
    F.get_or_init(|| {
        let s = fontdue::FontSettings::default();
        [
            fontdue::Font::from_bytes(REGULAR_TTF, s).expect("bundled regular font"),
            fontdue::Font::from_bytes(BOLD_TTF, fontdue::FontSettings::default()).expect("bundled bold font"),
        ]
    })
}

/// A logical font (subset of LOGFONTA).
#[derive(Debug, Clone, PartialEq)]
pub struct FontSpec {
    pub height: i32,
    pub width: i32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub charset: u8,
    pub pitch_family: u8,
    pub face: String,
}

impl Default for FontSpec {
    fn default() -> Self {
        FontSpec {
            height: 0,
            width: 0,
            weight: 400,
            italic: false,
            underline: false,
            strikeout: false,
            charset: 0,
            pitch_family: 0x22, // VARIABLE_PITCH | FF_SWISS
            face: "MS Sans Serif".into(),
        }
    }
}

/// Pixel metrics of a font at its resolved size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// fontdue px size (em).
    pub px: f32,
    pub ascent: i32,
    pub descent: i32,
    pub height: i32,
    pub internal_leading: i32,
    pub ave_char_width: i32,
    pub max_char_width: i32,
    pub bold: bool,
}

/// A rasterized glyph.
#[derive(Debug, Clone)]
pub struct Glyph {
    pub advance: i32,
    pub xmin: i32,
    /// Offset of the bitmap's top row relative to the baseline (positive = above).
    pub top: i32,
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<u8>,
}

/// Glyph cache keyed by (size*64, bold, char).
#[derive(Default)]
pub struct FontCache {
    glyphs: HashMap<(u32, bool, char), Glyph>,
}

/// Windows-1252 byte to char.
pub fn cp1252(b: u8) -> char {
    const HI: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D,
        0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A,
        0x0153, 0x9D, 0x017E, 0x0178,
    ];
    if (0x80..0xA0).contains(&b) {
        char::from_u32(HI[(b - 0x80) as usize] as u32).unwrap_or('?')
    } else {
        b as char
    }
}

/// Resolve a logical font to metrics.
pub fn metrics(spec: &FontSpec) -> Metrics {
    let bold = spec.weight >= 600;
    let f = &faces()[bold as usize];
    let lm1 = f.horizontal_line_metrics(1.0).expect("line metrics");
    let cell_per_em = lm1.ascent - lm1.descent; // descent is negative
    let px = if spec.height < 0 {
        (-spec.height) as f32
    } else if spec.height > 0 {
        spec.height as f32 / cell_per_em
    } else {
        13.0 / cell_per_em
    };
    let ascent = (lm1.ascent * px).round() as i32;
    let descent = (-lm1.descent * px).round() as i32;
    let height = ascent + descent;
    let internal_leading = (height - px.round() as i32).max(0);
    let adv = |c: char| f.metrics(c, px).advance_width.round() as i32;
    Metrics {
        px,
        ascent,
        descent,
        height,
        internal_leading,
        ave_char_width: adv('x'),
        max_char_width: adv('W').max(adv('M')),
        bold,
    }
}

impl FontCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rasterized glyph (cached).
    pub fn glyph(&mut self, mt: &Metrics, c: char) -> &Glyph {
        let key = ((mt.px * 64.0) as u32, mt.bold, c);
        self.glyphs.entry(key).or_insert_with(|| {
            let f = &faces()[mt.bold as usize];
            let (m, cov) = f.rasterize(c, mt.px);
            Glyph {
                advance: m.advance_width.round() as i32,
                xmin: m.xmin,
                top: m.ymin + m.height as i32,
                width: m.width,
                height: m.height,
                coverage: cov,
            }
        })
    }

    /// Advance of one char.
    pub fn advance(&mut self, mt: &Metrics, c: char) -> i32 {
        if c == '\t' {
            return self.advance(mt, ' ') * 8;
        }
        self.glyph(mt, c).advance
    }

    /// Width of a string (cp1252 bytes).
    pub fn text_width(&mut self, mt: &Metrics, text: &[u8]) -> i32 {
        text.iter().map(|&b| self.advance(mt, cp1252(b))).sum()
    }

    /// Draw `text` with its top-left cell corner at (x, y); `plot(x, y, coverage)` is called for each covered
    /// pixel. Returns the total advance.
    pub fn draw(&mut self, mt: &Metrics, text: &[u8], x: i32, y: i32, mut plot: impl FnMut(i32, i32, u8)) -> i32 {
        let baseline = y + mt.ascent;
        let mut pen = x;
        for &b in text {
            let c = cp1252(b);
            if c == '\t' {
                pen += self.advance(mt, c);
                continue;
            }
            let g = self.glyph(mt, c);
            let gx = pen + g.xmin;
            let gy = baseline - g.top;
            for row in 0..g.height {
                for col in 0..g.width {
                    let a = g.coverage[row * g.width + col];
                    if a != 0 {
                        plot(gx + col as i32, gy + row as i32, a);
                    }
                }
            }
            pen += g.advance;
        }
        pen - x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_and_draw() {
        let spec = FontSpec { height: 15, ..Default::default() };
        let mt = metrics(&spec);
        assert!((mt.height - 15).abs() <= 1, "{mt:?}");
        let mut fc = FontCache::new();
        let w = fc.text_width(&mt, b"Hello");
        assert!(w > 20 && w < 50, "width {w}");
        let mut n = 0;
        let mut maxy = 0;
        let adv = fc.draw(&mt, b"Hello", 0, 0, |_, y, _| {
            n += 1;
            maxy = maxy.max(y);
        });
        assert_eq!(adv, w);
        assert!(n > 30);
        assert!(maxy < mt.height);
        let neg = metrics(&FontSpec { height: -12, weight: 700, ..Default::default() });
        assert!(neg.bold && neg.px == 12.0);
        assert_eq!(cp1252(0xFC), 'ü');
        assert_eq!(cp1252(0x80), '€');
    }
}
