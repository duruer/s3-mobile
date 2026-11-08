//! Full-screen menu layouts (docs/porting/menu-layout.md).
//!
//! The game draws its menus as fixed 800x600 (4:3) frames. On a wide phone (e.g. 2316x904, ~2.56:1) that leaves
//! big black bars, and plain cropping (`Fill`) cuts away more than half of the rows. A [`MenuLayout`] instead
//! re-arranges parts of one known menu screen on a wide "canvas": every [`Piece`] copies a source rectangle of the
//! frame (game pixels) to a position on the canvas (canvas units = game pixels, one uniform scale for the whole
//! canvas). The canvas is fitted into the window; a background (stretched source rectangle or a colour) covers the
//! whole window first, so there are no black bars.
//!
//! Screens are recognised from the frame itself ([`detect`]): each layout lists a few probes (4x4 block means of
//! static decoration). An unknown frame (a popup over the menu, a screen without layout) returns `None` and the
//! frontend falls back to the normal display mode, so nothing ever becomes unusable.
//!
//! Touches map back through the same pieces ([`MenuLayout::canvas_to_game`]); a touch on the background goes to
//! the layout's `neutral` game point (a spot without buttons). Pure presentation: no guest memory is touched.

/// A source rectangle (game pixels) placed on the canvas at `at` (same size).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    /// x, y, w, h in game pixels.
    pub src: [u16; 4],
    /// Top-left corner on the canvas.
    pub at: [u16; 2],
}

/// What covers the window behind the pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    /// This source rectangle stretched over the whole window.
    Stretch([u16; 4]),
    /// A plain colour (r, g, b).
    Color([u8; 3]),
}

/// 4x4 block mean (r, g, b, 8 bit) expected at (x, y) (top-left of the block).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub x: u16,
    pub y: u16,
    pub rgb: [u8; 3],
}

/// Layout of one recognised menu screen.
#[derive(Debug, PartialEq, Eq)]
pub struct MenuLayout {
    pub name: &'static str,
    /// Frame size the layout is for.
    pub frame: [u16; 2],
    /// Canvas size (game-pixel units); fitted into the window keeping its aspect.
    pub canvas: [u16; 2],
    pub background: Background,
    /// Drawn in order (later pieces on top); touch hit test in reverse order.
    pub pieces: &'static [Piece],
    /// Game point for touches outside every piece.
    pub neutral: [u16; 2],
    /// Strips repeated vertically over the window bands above and below the canvas (a window taller than the
    /// canvas): `src` is one period of the strip, `at` the canvas position of one copy. Keeps edge decoration (the
    /// main menu's Greek-key borders) running along the whole window height (user report 2026-11-08).
    pub band_tiles: &'static [Piece],
    pub probes: &'static [Probe],
}

/// Per-channel tolerance of a probe (rgb565 rounding, small animations).
const PROBE_TOL: i32 = 14;
/// Probes allowed to miss (the mouse cursor drawn into the frame can cover one). Popups cover more.
const PROBE_MISSES: usize = 1;

impl MenuLayout {
    /// Canvas point -> game point (fractional). Outside every piece: the neutral point.
    pub fn canvas_to_game(&self, u: f32, v: f32) -> (f32, f32) {
        for p in self.pieces.iter().rev() {
            let (ax, ay) = (p.at[0] as f32, p.at[1] as f32);
            let (w, h) = (p.src[2] as f32, p.src[3] as f32);
            if u >= ax && v >= ay && u < ax + w && v < ay + h {
                return (p.src[0] as f32 + (u - ax), p.src[1] as f32 + (v - ay));
            }
        }
        (self.neutral[0] as f32 + 0.5, self.neutral[1] as f32 + 0.5)
    }

    /// Game point -> canvas point, if that game pixel is shown by a piece (the topmost one).
    pub fn game_to_canvas(&self, gx: f32, gy: f32) -> Option<(f32, f32)> {
        self.pieces.iter().rev().find_map(|p| {
            let (sx, sy) = (p.src[0] as f32, p.src[1] as f32);
            let (w, h) = (p.src[2] as f32, p.src[3] as f32);
            (gx >= sx && gy >= sy && gx < sx + w && gy < sy + h).then(|| (p.at[0] as f32 + gx - sx, p.at[1] as f32 + gy - sy))
        })
    }

    /// Do all probes match this RGB565 frame?
    pub fn matches(&self, px: &[u16], w: u32, h: u32) -> bool {
        if w != self.frame[0] as u32 || h != self.frame[1] as u32 || px.len() < (w * h) as usize {
            return false;
        }
        let misses = self
            .probes
            .iter()
            .filter(|p| {
                let m = block_mean(px, w as usize, p.x as usize, p.y as usize);
                !(0..3).all(|i| (m[i] as i32 - p.rgb[i] as i32).abs() <= PROBE_TOL)
            })
            .count();
        !self.probes.is_empty() && misses <= PROBE_MISSES
    }
}

/// 4x4 block mean (8-bit rgb) of an RGB565 frame at (x, y).
pub fn block_mean(px: &[u16], stride: usize, x: usize, y: usize) -> [u8; 3] {
    let mut s = [0u32; 3];
    for dy in 0..4 {
        for dx in 0..4 {
            let (r, g, b) = rgb565_to_888(px[(y + dy) * stride + x + dx]);
            s[0] += r;
            s[1] += g;
            s[2] += b;
        }
    }
    [(s[0] / 16) as u8, (s[1] / 16) as u8, (s[2] / 16) as u8]
}

fn rgb565_to_888(p: u16) -> (u32, u32, u32) {
    let r = ((p >> 11) & 31) as u32;
    let g = ((p >> 5) & 63) as u32;
    let b = (p & 31) as u32;
    ((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2))
}

/// The layout for this frame, if it is a known menu screen.
pub fn detect(px: &[u16], w: u32, h: u32) -> Option<&'static MenuLayout> {
    LAYOUTS.iter().copied().find(|l| l.matches(px, w, h))
}

/// Prefer the whole original screen ([`full_variant`]) when it can be shown at least this big relative to the
/// re-arranged layout (window px per game px). Tablets (4:3, 16:10), a foldable's inner screen (~1.2:1) and 16:9
/// phones then show every element of the menu; very wide screens (Fold cover ~2.56:1, 20:9) keep the layout,
/// which drops decoration to make the buttons bigger.
pub const FULL_PREFER: f32 = 0.8;

/// The layout to show for a detected screen in a `win_w` x `win_h` window: the re-arranged layout, or the whole
/// frame (all elements) when the window's shape lets it be nearly as big ([`FULL_PREFER`]). Responsive: called for
/// every frame with the current window, so folding / unfolding / resizing switches at once.
pub fn choose(l: &'static MenuLayout, win_w: u32, win_h: u32) -> &'static MenuLayout {
    let Some(full) = full_variant(l) else { return l };
    let s_layout = canvas_placement(l, win_w, win_h).2;
    let s_full = canvas_placement(full, win_w, win_h).2;
    if s_full >= FULL_PREFER * s_layout {
        full
    } else {
        l
    }
}

/// The whole-frame variant of a layout (same detection, background colour behind the bars), `None` when the
/// layout already shows the whole frame.
pub fn full_variant(l: &MenuLayout) -> Option<&'static MenuLayout> {
    if l.canvas == l.frame && l.pieces.len() == 1 {
        return None;
    }
    FULL_LAYOUTS.iter().copied().find(|f| f.name.strip_suffix("-full") == Some(l.name))
}

const FULL_800: [Piece; 1] = [Piece { src: [0, 0, 800, 600], at: [0, 0] }];

macro_rules! full {
    ($stat:ident, $base:ident, $name:literal) => {
        full!($stat, $base, $name, &[]);
    };
    ($stat:ident, $base:ident, $name:literal, $tiles:expr) => {
        pub static $stat: MenuLayout = MenuLayout {
            name: $name,
            frame: $base.frame,
            canvas: $base.frame,
            background: match $base.background {
                Background::Color(c) => Background::Color(c),
                Background::Stretch(_) => STONE_BG,
            },
            pieces: &FULL_800,
            neutral: $base.neutral,
            band_tiles: $tiles,
            probes: &[],
        };
    };
}

/// Greek-key borders of the main menu: the pattern repeats every 62 rows (measured on the 800x600 frame).
const MAIN_KEY_PERIOD: u16 = 62;
full!(
    MAIN_MENU_FULL,
    MAIN_MENU,
    "main-menu-full",
    &[pc(0, 100, 22, MAIN_KEY_PERIOD, 0, 100), pc(778, 100, 22, MAIN_KEY_PERIOD, 778, 100)]
);
full!(CHOOSE_RACE_FULL, CHOOSE_RACE, "choose-race-full");
full!(CHOOSE_RACE_MCD_FULL, CHOOSE_RACE_MCD, "choose-race-mcd-full");
full!(CHOOSE_RACE_AMAZON_FULL, CHOOSE_RACE_AMAZON, "choose-race-amazon-full");
full!(MAP_PICKER_FULL, MAP_PICKER, "map-picker-full");
full!(GAME_SETUP_FULL, GAME_SETUP, "game-setup-full");
full!(PLAYERS_FULL, PLAYERS, "players-full");
full!(LAN_GAME_FULL, LAN_GAME, "lan-game-full");
full!(LOAD_GAME_FULL, LOAD_GAME, "load-game-full");
full!(BRIEFING_FULL, BRIEFING, "briefing-full");

/// Whole-frame variants (not detected themselves; see [`choose`]).
pub static FULL_LAYOUTS: &[&MenuLayout] = &[
    &MAIN_MENU_FULL,
    &CHOOSE_RACE_FULL,
    &CHOOSE_RACE_MCD_FULL,
    &CHOOSE_RACE_AMAZON_FULL,
    &MAP_PICKER_FULL,
    &GAME_SETUP_FULL,
    &PLAYERS_FULL,
    &LAN_GAME_FULL,
    &LOAD_GAME_FULL,
    &BRIEFING_FULL,
];

/// Canvas placement in a window: (origin x, origin y, scale window px per canvas unit).
pub fn canvas_placement(l: &MenuLayout, win_w: u32, win_h: u32) -> (f32, f32, f32) {
    let (cw, ch) = (l.canvas[0] as f32, l.canvas[1] as f32);
    let s = (win_w as f32 / cw).min(win_h as f32 / ch);
    (((win_w as f32 - cw * s) / 2.0).round(), ((win_h as f32 - ch * s) / 2.0).round(), s)
}

/// Copies of the layout's [`MenuLayout::band_tiles`] outside the canvas rows, as (source rect in game px, window
/// rect) pairs; partial copies are cut at the canvas edge and the window edge.
pub fn band_quads(l: &MenuLayout, win_w: u32, win_h: u32) -> Vec<([f32; 4], [f32; 4])> {
    let mut out = Vec::new();
    let (ox, oy, s) = canvas_placement(l, win_w, win_h);
    if s <= 0.0 {
        return out;
    }
    let ch = l.canvas[1] as f32;
    // Window rows above / below the canvas, in canvas units.
    let bands = [(-oy / s, 0.0), (ch, ch + (win_h as f32 - oy - ch * s) / s)];
    for t in l.band_tiles {
        let [sx, sy, sw, sh] = t.src.map(|v| v as f32);
        if sh < 1.0 {
            continue;
        }
        for &(b0, b1) in &bands {
            if b1 - b0 < 0.01 {
                continue;
            }
            // Copies k: canvas rows at.y + k*sh .. + sh, cut to the band.
            let base = t.at[1] as f32;
            let mut k = ((b0 - base) / sh).floor();
            loop {
                let c0 = base + k * sh;
                if c0 >= b1 {
                    break;
                }
                let (v0, v1) = (c0.max(b0), (c0 + sh).min(b1));
                if v1 > v0 {
                    let src = [sx, sy + (v0 - c0), sw, v1 - v0];
                    let dst = [ox + t.at[0] as f32 * s, oy + v0 * s, sw * s, (v1 - v0) * s];
                    out.push((src, dst));
                }
                k += 1.0;
            }
        }
    }
    out
}

/// CPU renderer (desktop window, Android CPU fallback, previews): nearest sampling into XRGB8888.
pub fn render_xrgb(l: &MenuLayout, src: &[u16], gw: usize, gh: usize, dst: &mut [u32], ww: usize, wh: usize) {
    if src.len() < gw * gh || dst.len() < ww * wh {
        return;
    }
    let px = |x: usize, y: usize| {
        let (r, g, b) = rgb565_to_888(src[y.min(gh - 1) * gw + x.min(gw - 1)]);
        (r << 16) | (g << 8) | b
    };
    match l.background {
        Background::Color([r, g, b]) => dst[..ww * wh].fill(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
        Background::Stretch([sx, sy, sw, sh]) => {
            for y in 0..wh {
                let gy = sy as usize + y * sh as usize / wh;
                for x in 0..ww {
                    dst[y * ww + x] = px(sx as usize + x * sw as usize / ww, gy);
                }
            }
        }
    }
    for (sr, d) in band_quads(l, ww as u32, wh as u32) {
        let (x0, y0) = (d[0].round().max(0.0) as usize, d[1].round().max(0.0) as usize);
        let x1 = ((d[0] + d[2]).round().max(0.0) as usize).min(ww);
        let y1 = ((d[1] + d[3]).round().max(0.0) as usize).min(wh);
        for y in y0..y1 {
            let v = (sr[1] + (y as f32 + 0.5 - d[1]) * sr[3] / d[3]).clamp(sr[1], sr[1] + sr[3] - 0.01) as usize;
            for x in x0..x1 {
                let u = (sr[0] + (x as f32 + 0.5 - d[0]) * sr[2] / d[2]).clamp(sr[0], sr[0] + sr[2] - 0.01) as usize;
                dst[y * ww + x] = px(u, v);
            }
        }
    }
    let (ox, oy, s) = canvas_placement(l, ww as u32, wh as u32);
    for p in l.pieces {
        let x0 = (ox + p.at[0] as f32 * s).round().max(0.0) as usize;
        let y0 = (oy + p.at[1] as f32 * s).round().max(0.0) as usize;
        let x1 = ((ox + (p.at[0] + p.src[2]) as f32 * s).round() as usize).min(ww);
        let y1 = ((oy + (p.at[1] + p.src[3]) as f32 * s).round() as usize).min(wh);
        for y in y0..y1 {
            let v = ((y as f32 + 0.5 - oy) / s - p.at[1] as f32).clamp(0.0, p.src[3] as f32 - 1.0) as usize;
            for x in x0..x1 {
                let u = ((x as f32 + 0.5 - ox) / s - p.at[0] as f32).clamp(0.0, p.src[2] as f32 - 1.0) as usize;
                dst[y * ww + x] = px(p.src[0] as usize + u, p.src[1] as usize + v);
            }
        }
    }
}

const fn pc(x: u16, y: u16, w: u16, h: u16, ax: u16, ay: u16) -> Piece {
    Piece { src: [x, y, w, h], at: [ax, ay] }
}

const fn pr(x: u16, y: u16, r: u8, g: u8, b: u8) -> Probe {
    Probe { x, y, rgb: [r, g, b] }
}

// ---------------------------------------------------------------- the layouts (GOG S3.EXE 1.60, 800x600 menus)


/// Behind the stone screens. A plain colour, not a tile of the frame: the game draws the mouse cursor into the
/// frame, so a tiled or stretched source rectangle would repeat the cursor all over the window.
const STONE_BG: Background = Background::Color([44, 58, 72]);

/// All known layouts, tried in order.
pub static LAYOUTS: &[&MenuLayout] = &[
    &MAIN_MENU,
    &CHOOSE_RACE,
    &CHOOSE_RACE_MCD,
    &CHOOSE_RACE_AMAZON,
    &MAP_PICKER,
    &GAME_SETUP,
    &PLAYERS,
    &LAN_GAME,
    &LOAD_GAME,
    &BRIEFING,
    &LOADING,
];

/// Main menu: the 13 buttons in two columns (the original gap before "Online Help" / "Exit Game" kept), the
/// "Settlers III Gold Edition" laurel on the right; helmets, Blue Byte logo and version text dropped.
pub static MAIN_MENU: MenuLayout = MenuLayout {
    name: "main-menu",
    frame: [800, 600],
    canvas: [860, 330],
    background: Background::Color([165, 150, 82]),
    pieces: &[
        // Greek-key borders.
        pc(0, 0, 22, 330, 0, 0),
        pc(778, 0, 22, 330, 838, 0),
        // Column 1: Tutorial .. Multi-player Game: LAN.
        pc(78, 18, 176, 276, 44, 27),
        // Column 2: Internet + Load Game, Online Help .. Credits, Exit Game.
        pc(78, 298, 176, 76, 240, 27),
        pc(78, 398, 176, 116, 240, 127),
        pc(78, 538, 176, 36, 240, 267),
        // Laurel + logo.
        pc(300, 0, 400, 280, 436, 25),
    ],
    neutral: [270, 590],
    band_tiles: &[pc(0, 0, 22, MAIN_KEY_PERIOD, 0, 0), pc(778, 0, 22, MAIN_KEY_PERIOD, 838, 0)],
    probes: &[
        pr(8, 300, 68, 59, 65),
        pr(785, 300, 33, 28, 57),
        pr(285, 230, 165, 150, 82),
        pr(270, 560, 165, 150, 82),
        pr(500, 115, 185, 167, 118),
        pr(380, 200, 55, 48, 19),
    ],
};

/// "Choose a Race" (Campaign): the three race cards side by side under the title; helmets dropped.
pub static CHOOSE_RACE: MenuLayout = MenuLayout {
    name: "choose-race",
    frame: [800, 600],
    canvas: [840, 330],
    background: STONE_BG,
    pieces: &[
        pc(298, 34, 214, 30, 313, 6),
        pc(53, 163, 186, 236, 24, 60),
        pc(238, 363, 237, 180, 302, 80),
        pc(563, 198, 207, 201, 610, 64),
    ],
    neutral: [400, 580],
    band_tiles: &[],
    probes: &[
        pr(370, 44, 130, 64, 40),
        pr(400, 250, 34, 39, 42),
        pr(650, 480, 49, 71, 88),
        pr(200, 450, 96, 131, 162),
    ],
};

/// "Choose a Race" (Mission CD): a band of the painting, title and labels moved into it.
pub static CHOOSE_RACE_MCD: MenuLayout = MenuLayout {
    name: "choose-race-mcd",
    frame: [800, 600],
    canvas: [800, 312],
    background: Background::Color([8, 20, 30]),
    pieces: &[
        pc(0, 60, 800, 312, 0, 0),
        pc(282, 531, 234, 36, 283, 4),
        pc(44, 419, 112, 44, 40, 262),
        pc(571, 384, 111, 44, 571, 262),
    ],
    neutral: [60, 20],
    band_tiles: &[],
    probes: &[
        pr(330, 545, 11, 60, 60),
        pr(300, 100, 7, 0, 0),
        pr(700, 30, 46, 14, 0),
        pr(300, 300, 38, 98, 105),
    ],
};

/// "Choose a Race" (Amazon campaign): a band of the painting, title and labels moved into it.
pub static CHOOSE_RACE_AMAZON: MenuLayout = MenuLayout {
    name: "choose-race-amazon",
    frame: [800, 600],
    canvas: [800, 312],
    background: Background::Color([20, 30, 60]),
    pieces: &[
        pc(0, 90, 800, 312, 0, 0),
        pc(289, 23, 222, 42, 289, 4),
        pc(60, 394, 112, 28, 60, 278),
        pc(611, 486, 112, 28, 611, 278),
    ],
    neutral: [650, 10],
    band_tiles: &[],
    probes: &[
        pr(340, 40, 145, 74, 83),
        pr(400, 150, 47, 75, 164),
        pr(400, 560, 9, 3, 2),
        pr(700, 300, 44, 44, 0),
    ],
};

/// Map picker popup over Game Setup (Random / Single / Multi-player / User maps).
pub static MAP_PICKER: MenuLayout = MenuLayout {
    name: "map-picker",
    frame: [800, 600],
    canvas: [870, 340],
    background: STONE_BG,
    pieces: &[
        // Tabs + map list.
        pc(115, 117, 131, 330, 10, 5),
        // Options / map preview.
        pc(248, 117, 442, 330, 150, 5),
        // Cancel / OK.
        pc(455, 452, 230, 32, 630, 300),
    ],
    neutral: [400, 590],
    band_tiles: &[],
    probes: &[
        pr(102, 300, 134, 147, 163),
        pr(692, 300, 48, 63, 76),
        pr(400, 40, 106, 74, 66),
        pr(400, 490, 100, 133, 151),
    ],
};

/// Game Setup (single player scenario, multiplayer host): blocks re-flowed into three columns.
pub static GAME_SETUP: MenuLayout = MenuLayout {
    name: "game-setup",
    frame: [800, 600],
    canvas: [930, 340],
    background: STONE_BG,
    pieces: &[
        // Title + name of the game.
        pc(248, 28, 366, 80, 282, 4),
        // Map settings, Common team setups.
        pc(16, 133, 369, 129, 10, 92),
        pc(16, 293, 369, 94, 10, 228),
        // Team settings: general, Cancel / OK.
        pc(16, 418, 302, 129, 390, 92),
        pc(556, 553, 228, 30, 464, 300),
        // Team settings: specific.
        pc(568, 133, 219, 240, 705, 92),
    ],
    neutral: [450, 300],
    band_tiles: &[],
    probes: &[
        pr(390, 40, 142, 76, 53),
        pr(30, 145, 211, 79, 23),
        pr(30, 305, 68, 54, 54),
        pr(450, 300, 68, 88, 107),
        pr(650, 300, 22, 30, 34),
        pr(500, 450, 100, 126, 143),
    ],
};

/// Player list (after Game Setup; also the multiplayer waiting room).
pub static PLAYERS: MenuLayout = MenuLayout {
    name: "players",
    frame: [800, 600],
    canvas: [860, 336],
    background: STONE_BG,
    pieces: &[
        pc(18, 15, 292, 55, 10, 6),
        pc(498, 15, 285, 55, 565, 6),
        pc(16, 84, 768, 200, 46, 66),
        pc(16, 516, 254, 24, 10, 270),
        pc(16, 553, 349, 29, 10, 300),
        pc(436, 553, 348, 29, 502, 300),
    ],
    neutral: [400, 560],
    band_tiles: &[],
    probes: &[
        pr(40, 40, 103, 91, 57),
        pr(760, 50, 16, 24, 24),
        pr(400, 300, 24, 31, 34),
        pr(400, 560, 74, 112, 140),
    ],
};

/// LAN Game: the two list rows on the left, title / status / Back / Join / New as a column on the right.
pub static LAN_GAME: MenuLayout = MenuLayout {
    name: "lan-game",
    frame: [800, 600],
    canvas: [880, 340],
    background: STONE_BG,
    pieces: &[
        pc(124, 86, 276, 166, 8, 4),
        pc(450, 86, 216, 140, 334, 4),
        pc(124, 282, 276, 164, 8, 174),
        pc(448, 282, 282, 164, 332, 174),
        pc(310, 26, 214, 30, 640, 8),
        pc(126, 488, 258, 32, 618, 60),
        pc(438, 553, 104, 30, 695, 120),
        pc(558, 553, 104, 30, 695, 165),
        pc(678, 553, 104, 30, 695, 210),
    ],
    neutral: [420, 520],
    band_tiles: &[],
    probes: &[
        pr(390, 40, 127, 66, 44),
        pr(140, 96, 82, 88, 57),
        pr(400, 250, 52, 68, 85),
        pr(420, 520, 93, 132, 154),
        pr(420, 200, 90, 117, 133),
    ],
};

/// Load Game: the upper part of the list, buttons on the right.
pub static LOAD_GAME: MenuLayout = MenuLayout {
    name: "load-game",
    frame: [800, 600],
    canvas: [870, 340],
    background: STONE_BG,
    pieces: &[
        pc(194, 94, 414, 330, 120, 5),
        pc(310, 26, 214, 30, 560, 10),
        pc(636, 98, 108, 68, 600, 70),
        pc(676, 553, 108, 30, 600, 300),
    ],
    neutral: [690, 300],
    band_tiles: &[],
    probes: &[
        pr(390, 40, 132, 62, 35),
        pr(400, 300, 36, 43, 44),
        pr(690, 300, 36, 52, 64),
        pr(500, 570, 101, 133, 151),
    ],
};

/// Campaign mission briefing: text panel left, map right, tip text and Cancel / OK below.
pub static BRIEFING: MenuLayout = MenuLayout {
    name: "briefing",
    frame: [800, 600],
    canvas: [900, 340],
    background: STONE_BG,
    pieces: &[
        pc(83, 58, 621, 234, 8, 6),
        pc(73, 343, 254, 170, 640, 6),
        pc(331, 331, 372, 92, 8, 246),
        pc(556, 553, 228, 30, 650, 300),
    ],
    neutral: [740, 400],
    band_tiles: &[],
    probes: &[
        pr(695, 150, 79, 93, 112),
        pr(20, 320, 102, 136, 166),
        pr(400, 565, 109, 144, 165),
        pr(740, 400, 44, 63, 81),
        pr(520, 480, 33, 40, 43),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_map_both_ways() {
        let l = &MAIN_MENU;
        // "Load Game" (game 165, 355) sits in column 2.
        let (u, v) = l.game_to_canvas(165.0, 355.0).unwrap();
        assert_eq!((u, v), (240.0 + 87.0, 27.0 + 57.0));
        assert_eq!(l.canvas_to_game(u, v), (165.0, 355.0));
        // Background -> neutral point; helmets are not shown.
        assert_eq!(l.canvas_to_game(430.0, 320.0), (270.5, 590.5));
        assert_eq!(l.game_to_canvas(400.0, 450.0), None);
    }

    #[test]
    fn layouts_are_sane() {
        for l in LAYOUTS {
            assert!(!l.probes.is_empty(), "{}", l.name);
            for p in l.pieces {
                assert!(p.src[0] + p.src[2] <= l.frame[0] && p.src[1] + p.src[3] <= l.frame[1], "{} src {:?}", l.name, p.src);
                assert!(p.at[0] + p.src[2] <= l.canvas[0] && p.at[1] + p.src[3] <= l.canvas[1], "{} at {:?}", l.name, p);
            }
            for p in l.probes {
                assert!(p.x + 4 <= l.frame[0] && p.y + 4 <= l.frame[1], "{} probe {p:?}", l.name);
            }
            assert!(l.neutral[0] < l.frame[0] && l.neutral[1] < l.frame[1]);
        }
    }

    #[test]
    fn detection_needs_matching_probes() {
        let black = vec![0u16; 800 * 600];
        assert!(detect(&black, 800, 600).is_none());
        // A frame painted with the main menu probe colours (one probe may miss: the cursor).
        let mut f = black.clone();
        for p in MAIN_MENU.probes.iter().skip(1) {
            let c = ((p.rgb[0] as u16 >> 3) << 11) | ((p.rgb[1] as u16 >> 2) << 5) | (p.rgb[2] as u16 >> 3);
            for y in p.y as usize..p.y as usize + 4 {
                for x in p.x as usize..p.x as usize + 4 {
                    f[y * 800 + x] = c;
                }
            }
        }
        assert_eq!(detect(&f, 800, 600).map(|l| l.name), Some("main-menu"));
        assert!(detect(&f, 640, 480).is_none());
    }

    #[test]
    fn responsive_choice() {
        // Fold4 cover (2.56:1): the re-arranged layout; inner screen, 4:3 / 16:10 tablets, 16:9: the whole menu.
        assert_eq!(choose(&MAIN_MENU, 2316, 904).name, "main-menu");
        assert_eq!(choose(&MAIN_MENU, 2176, 1812).name, "main-menu-full");
        assert_eq!(choose(&MAIN_MENU, 2048, 1536).name, "main-menu-full");
        assert_eq!(choose(&MAIN_MENU, 2560, 1600).name, "main-menu-full");
        assert_eq!(choose(&MAIN_MENU, 1920, 1080).name, "main-menu-full");
        assert_eq!(choose(&MAIN_MENU, 2400, 1080).name, "main-menu");
        // Every layout has a full variant; the loading screen is already whole.
        for l in LAYOUTS {
            if l.name != "loading" {
                assert!(full_variant(l).is_some(), "{}", l.name);
            }
        }
        assert!(full_variant(&LOADING).is_none());
        // The full variant maps 1:1.
        let f = full_variant(&MAIN_MENU).unwrap();
        assert_eq!(f.game_to_canvas(400.0, 450.0), Some((400.0, 450.0)));
    }

    #[test]
    fn viewport_with_layout_covers_the_window() {
        use crate::display::{DisplayMode, Viewport};
        let vp = Viewport::with_layout(DisplayMode::NativeWide, &MAIN_MENU, 2316, 904);
        assert_eq!(vp.bars(), (0, 0, 0, 0));
        assert_eq!(vp.visible_game_rect(), (0, 0, 800, 600));
        let (wx, wy) = vp.game_to_window(165.0, 355.0);
        let (gx, gy) = vp.window_to_game(wx + 1.0, wy + 1.0);
        assert!((gx - 165).abs() <= 1 && (gy - 355).abs() <= 1, "{gx},{gy}");
    }
}

/// Loading screen before a map (640x480: laurel + helmets, hourglass top left). Kept whole; the sides are filled
/// with its own beige instead of black.
pub static LOADING: MenuLayout = MenuLayout {
    name: "loading",
    frame: [640, 480],
    canvas: [640, 480],
    background: Background::Color([165, 150, 82]),
    pieces: &[pc(0, 0, 640, 480, 0, 0)],
    neutral: [30, 470],
    band_tiles: &[],
    probes: &[
        pr(30, 5, 165, 150, 82),
        pr(610, 470, 165, 150, 82),
        pr(320, 250, 136, 90, 38),
        pr(300, 110, 164, 33, 33),
        pr(200, 440, 169, 153, 108),
        pr(450, 30, 189, 170, 123),
    ],
};
