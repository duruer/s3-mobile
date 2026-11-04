//! Status screen: a test pattern plus text (engine state, errors, "S3.EXE missing" instructions), so the app
//! never shows a plain black screen. Also used as an overlay on the last game frame when the engine stops.

use s3_runtime::FrameBuffer;

pub const STATUS_W: u32 = 800;
pub const STATUS_H: u32 = 600;

#[inline]
pub const fn rgb(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

pub const WHITE: u16 = rgb(255, 255, 255);
pub const BLACK: u16 = 0;
pub const YELLOW: u16 = rgb(255, 230, 80);
pub const RED: u16 = rgb(255, 90, 90);

/// SMPTE-like colour bars + a 50 px grid + corner markers (checks letterboxing and touch mapping).
pub fn test_pattern(w: u32, h: u32) -> FrameBuffer {
    let mut fb = FrameBuffer::new(w, h);
    const BARS: [u16; 8] = [
        rgb(192, 192, 192),
        rgb(192, 192, 0),
        rgb(0, 192, 192),
        rgb(0, 192, 0),
        rgb(192, 0, 192),
        rgb(192, 0, 0),
        rgb(0, 0, 192),
        rgb(16, 16, 16),
    ];
    for y in 0..h {
        for x in 0..w {
            let mut c = if y < h / 3 {
                BARS[(x * 8 / w) as usize]
            } else {
                // Dark blue background with a gradient.
                rgb(0, (y * 64 / h) as u8, 48 + (x * 64 / w) as u8)
            };
            if x % 50 == 0 || y % 50 == 0 {
                c = rgb(60, 60, 90);
            }
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                c = WHITE;
            }
            fb.pixels[(y * w + x) as usize] = c;
        }
    }
    // Corner squares: red top-left, green top-right, blue bottom-left, white bottom-right.
    let sq = 24;
    fill_rect(&mut fb, 0, 0, sq, sq, rgb(255, 0, 0));
    fill_rect(&mut fb, w - sq, 0, sq, sq, rgb(0, 255, 0));
    fill_rect(&mut fb, 0, h - sq, sq, sq, rgb(0, 0, 255));
    fill_rect(&mut fb, w - sq, h - sq, sq, sq, WHITE);
    fb
}

pub fn fill_rect(fb: &mut FrameBuffer, x: u32, y: u32, w: u32, h: u32, c: u16) {
    for yy in y..(y + h).min(fb.height) {
        for xx in x..(x + w).min(fb.width) {
            fb.pixels[(yy * fb.width + xx) as usize] = c;
        }
    }
}

/// Draw one line of ASCII text with the 8x8 font scaled by `scale`. Returns the x after the text.
pub fn draw_text(fb: &mut FrameBuffer, x: i32, y: i32, text: &str, scale: u32, color: u16) -> i32 {
    let mut cx = x;
    for ch in text.chars() {
        let code = ch as u32;
        let glyph = if code < 128 { font8x8::legacy::BASIC_LEGACY[code as usize] } else { font8x8::legacy::BASIC_LEGACY[b'?' as usize] };
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if bits >> col & 1 == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = cx + (col * scale + sx) as i32;
                        let py = y + (row as u32 * scale + sy) as i32;
                        if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                            fb.pixels[(py as u32 * fb.width + px as u32) as usize] = color;
                        }
                    }
                }
            }
        }
        cx += 8 * scale as i32;
    }
    cx
}

/// Word-wrap `text` to at most `cols` characters per line (also splits on '\n').
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let mut word = word.to_string();
            while word.len() > cols {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                let rest = word.split_off(cols);
                out.push(word);
                word = rest;
            }
            if line.len() + word.len() + usize::from(!line.is_empty()) > cols {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        out.push(line);
    }
    out
}

/// Draw a text box (dark background) with wrapped lines; `title` in yellow. Anchored at the bottom if
/// `bottom`, else at the top-left below the colour bars.
pub fn draw_panel(fb: &mut FrameBuffer, title: &str, body: &str, bottom: bool) {
    let margin = 12u32;
    let avail_h = if bottom { fb.height / 2 } else { fb.height - fb.height / 3 - 16 };
    // Large font if the text fits, otherwise the small one.
    let mut scale = if fb.width >= 640 { 2 } else { 1 };
    let layout = |scale: u32| {
        let cols = ((fb.width - 2 * margin) / (8 * scale)).max(8) as usize;
        let max_lines = ((avail_h - 2 * margin) / (10 * scale)).max(2) as usize - 1;
        (wrap(body, cols), max_lines)
    };
    let (mut lines, mut max_lines) = layout(scale);
    if scale > 1 && lines.len() > max_lines {
        scale = 1;
        (lines, max_lines) = layout(1);
    }
    lines.truncate(max_lines);
    let line_h = 10 * scale;
    let box_h = (lines.len() as u32 + 1) * line_h + 2 * margin;
    let y0 = if bottom { fb.height.saturating_sub(box_h) } else { fb.height / 3 + 8 };
    fill_rect(fb, 0, y0, fb.width, box_h.min(fb.height - y0), rgb(8, 8, 24));
    let mut y = (y0 + margin) as i32;
    draw_text(fb, margin as i32, y, title, scale, YELLOW);
    y += line_h as i32;
    for (i, l) in lines.iter().enumerate() {
        draw_text(fb, margin as i32, y, l, scale, if i == 0 { RED } else { WHITE });
        y += line_h as i32;
    }
}

/// Full status screen: test pattern + panel.
pub fn status_screen(title: &str, body: &str) -> FrameBuffer {
    let mut fb = test_pattern(STATUS_W, STATUS_H);
    draw_text(&mut fb, 32, 8, "Settlers 3 Port", 3, BLACK);
    draw_panel(&mut fb, title, body, false);
    fb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_splits_words_and_long_tokens() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
    }

    #[test]
    fn text_draws_pixels() {
        let mut fb = FrameBuffer::new(64, 16);
        let end = draw_text(&mut fb, 0, 0, "A", 1, WHITE);
        assert_eq!(end, 8);
        assert!(fb.pixels.iter().any(|&p| p == WHITE));
    }

    #[test]
    fn status_screen_has_size() {
        let fb = status_screen("Engine stopped", &"x ".repeat(2000));
        assert_eq!((fb.width, fb.height, fb.pixels.len()), (800, 600, 480000));
    }
}
