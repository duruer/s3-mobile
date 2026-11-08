//! Pure model of the original left panel for UI mode A ("Classic Touch"): screen layout globals, the
//! button grid per page, the build list per race, and "which button does this finger mean" (snapping).
//! All addresses/grid values are for the GOG S3.EXE (docs/reverse-engineering/game-state-ui.md §1-2).
//! No guest code runs here; everything is plain data so it can be unit tested.

use s3_mem::GuestMemory;

/// Screen layout globals (set by `0x4c6d70(resIndex)`).
pub mod addr {
    pub const RES_INDEX1: u32 = 0x730f08;
    pub const SCREEN_W: u32 = 0x730f0c;
    pub const SCREEN_H: u32 = 0x730f10;
    pub const PANEL_W: u32 = 0x730f24;
    pub const MINIMAP_BOTTOM: u32 = 0x730f34;
    pub const CELL: u32 = 0x730f38;
    pub const GRID_X: u32 = 0x730f3c;
    pub const GRID_Y: u32 = 0x730f40;
}

/// Panel geometry for the current resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub screen_w: i32,
    pub screen_h: i32,
    pub panel_w: i32,
    pub minimap_bottom: i32,
    pub cell: i32,
    pub grid_x: i32,
    pub grid_y: i32,
}

impl Layout {
    /// The three layouts the game knows (640x480, 800x600, 1024x768), VERIFIED values.
    pub const fn builtin(res: u32) -> Layout {
        match res {
            0 => Layout { screen_w: 640, screen_h: 480, panel_w: 136, minimap_bottom: 142, cell: 3, grid_x: 8, grid_y: 152 },
            1 => Layout { screen_w: 800, screen_h: 600, panel_w: 176, minimap_bottom: 170, cell: 4, grid_x: 8, grid_y: 171 },
            _ => Layout { screen_w: 1024, screen_h: 768, panel_w: 216, minimap_bottom: 224, cell: 5, grid_x: 8, grid_y: 228 },
        }
    }

    /// Read the layout globals; `None` until the game has set them (index 1..3) or when they look wrong.
    pub fn read(mem: &GuestMemory) -> Option<Layout> {
        let idx = mem.read_u32(addr::RES_INDEX1);
        if !(1..=3).contains(&idx) {
            return None;
        }
        let l = Layout {
            screen_w: mem.read_u32(addr::SCREEN_W) as i32,
            screen_h: mem.read_u32(addr::SCREEN_H) as i32,
            panel_w: mem.read_u32(addr::PANEL_W) as i32,
            minimap_bottom: mem.read_u32(addr::MINIMAP_BOTTOM) as i32,
            cell: mem.read_u32(addr::CELL) as i32,
            grid_x: mem.read_u32(addr::GRID_X) as i32,
            grid_y: mem.read_u32(addr::GRID_Y) as i32,
        };
        let sane = (1..=8).contains(&l.cell)
            && (crate::res::MIN_W as i32..=crate::res::MAX_W as i32).contains(&l.screen_w)
            && (crate::res::MIN_H as i32..=crate::res::MAX_H as i32).contains(&l.screen_h)
            && (40..l.screen_w / 2).contains(&l.panel_w);
        sane.then_some(l)
    }

    /// Pixel rectangle (x, y, w, h) of a grid button (inclusive column/row ranges).
    pub fn rect(&self, b: &Button) -> (i32, i32, i32, i32) {
        let (c0, c1) = b.cols;
        let (r0, r1) = b.rows;
        (
            self.grid_x + c0 as i32 * self.cell,
            self.grid_y + r0 as i32 * self.cell,
            (c1 as i32 - c0 as i32 + 1) * self.cell,
            (r1 as i32 - r0 as i32 + 1) * self.cell,
        )
    }

    /// Is (x, y) in the panel button area (left panel below the minimap)?
    pub fn in_panel(&self, x: f32, y: f32) -> bool {
        x >= 0.0 && x < self.panel_w as f32 && y >= self.minimap_bottom as f32 && y < self.screen_h as f32
    }
}

/// What a button is called (for the magnifier bubble).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// A text id of the game's string tables (`table << 16 | index`), with an English fallback.
    Text(u32, &'static str),
    /// A building type (name = text `building_text_id(type)`).
    Building(u8),
}

/// A panel button in grid units (inclusive ranges, as in the game's hit tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub cols: (u8, u8),
    pub rows: (u8, u8),
    pub label: Label,
}

const fn b(c0: u8, c1: u8, r0: u8, r1: u8, label: Label) -> Button {
    Button { cols: (c0, c1), rows: (r0, r1), label }
}

/// Panel state needed to know which buttons exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelState {
    /// `[gd+0x2d284]`: 1 build, 2 goods, 3 settlers, 4 building, 5 units, 7 Save/Quit, 8 messages, 9 paused.
    pub page: u32,
    /// `[gd+0x2d298]`: menu sub-page on page 7.
    pub sub: u32,
    /// `[gd+0x2d288]`: build category 1..4.
    pub category: u32,
    /// Race of the local player (0..3).
    pub race: u8,
}

impl PanelState {
    /// Pages whose whole button set we know: snapping (moving a tap to the nearest button) is safe there.
    /// On other pages a tap is only snapped when it already lies inside a known button.
    pub fn fully_known(&self) -> bool {
        matches!(self.page, 0..=3) || (self.page == 7 && self.sub == 0)
    }
}

/// Building type in build list `slot` (1..12) of category `cat` (1..4) for `race` (0..3), `None` = empty
/// (STATIC table from `0x526f8a..0x528a37`, game-state-ui.md §2).
pub fn build_slot_type(cat: u32, slot: u32, race: u8) -> Option<u8> {
    const N: u8 = 0;
    let per_race = |t: [u8; 4]| t.get(race as usize).copied().filter(|&v| v != N);
    let one = |t: u8| Some(t);
    match (cat, slot) {
        (1, 1) => one(0x02),
        (1, 2) => one(0x04),
        (1, 3) => one(0x03),
        (1, 4) => one(0x05),
        (1, 5) => one(0x09),
        (1, 6) => one(0x0b),
        (1, 7) => one(0x08),
        (1, 8) => one(0x0a),
        (1, 9) => one(0x07),
        (1, 10) => one(0x0c),
        (1, 11) => per_race([0x27, 0x25, 0x1d, 0x38]),
        (1, 12) => per_race([N, N, 0x28, 0x25]),
        (2, 1) => one(0x19),
        (2, 2) => one(0x18),
        (2, 3) => one(0x17),
        (2, 4) => one(0x11),
        (2, 5) => one(0x15),
        (2, 6) => one(0x14),
        (2, 7) => one(0x1e),
        (2, 8) => one(0x31),
        (2, 9) => per_race([0x0e, 0x26, 0x16, 0x33]),
        (2, 10) => per_race([N, N, 0x24, 0x34]),
        (3, 1) => one(0x0f),
        (3, 2) => one(0x10),
        (3, 3) => one(0x12),
        (3, 4) => one(0x06),
        (3, 5) => one(0x0d),
        (3, 6) => per_race([0x1f, 0x2f, 0x30, 0x32]),
        (3, 7) => one(0x13),
        (3, 8) => one(0x20),
        (3, 9) => one(0x23),
        (4, 1) => one(0x1a),
        (4, 2) => one(0x1b),
        (4, 3) => one(0x1c),
        (4, 4) => one(0x01),
        (4, 5) => one(0x22),
        (4, 6) => one(0x21),
        (4, 7) => per_race([0x2c, 0x2a, 0x2e, 0x36]),
        (4, 8) => per_race([0x2b, 0x29, 0x2d, 0x37]),
        (4, 9) => per_race([N, N, N, 0x35]),
        _ => None,
    }
}

/// Text id of a building type's name (table 8). Types 0x32.. (Amazon variants) use other ids.
pub fn building_text_id(t: u8) -> u32 {
    match t {
        0x32 => 0x8015e,
        0x33 => 0x8015f,
        0x34 => 0x80160,
        0x35 => 0x80161,
        // Amazon variants of buildings other races have: reuse those names (HYPOTHESIS: same names in game).
        0x36 => 0x8002c,
        0x37 => 0x8002b,
        0x38 => 0x8001d,
        t => 0x80000 + t as u32,
    }
}

/// Buttons present on the panel for `st` (only the ones we know; see game-state-ui.md grid map).
pub fn buttons(st: &PanelState) -> Vec<Button> {
    let mut v = Vec::with_capacity(24);
    // Bottom row (menu + four message categories): every page.
    v.push(b(0, 7, 97, 103, Label::Text(0x8008f, "Save/Quit")));
    v.push(b(8, 15, 97, 103, Label::Text(0x800ad, "Battle Messages")));
    v.push(b(16, 23, 97, 103, Label::Text(0x800ae, "General Messages")));
    v.push(b(24, 31, 97, 103, Label::Text(0x800af, "Resource Messages")));
    v.push(b(32, 39, 97, 103, Label::Text(0x800b0, "Trade Messages")));
    if st.page == 7 {
        if st.sub == 0 {
            v.push(b(3, 37, 22, 29, Label::Text(0, "Save Game")));
            v.push(b(3, 37, 87, 91, Label::Text(0, "Quit Game")));
        }
        return v;
    }
    // Top tabs.
    v.push(b(2, 13, 1, 8, Label::Text(0x8004a, "Buildings")));
    v.push(b(15, 26, 1, 8, Label::Text(0x8004b, "Goods")));
    v.push(b(28, 39, 1, 8, Label::Text(0x8004c, "Settlers")));
    let quarters = |v: &mut Vec<Button>, ids: [(u32, &'static str); 4]| {
        for (i, (id, en)) in ids.into_iter().enumerate() {
            let c0 = (i * 10) as u8;
            v.push(b(c0, c0 + 9, 11, 18, Label::Text(id, en)));
        }
    };
    match st.page {
        1 => {
            quarters(&mut v, [
                (0x8004d, "Resource Buildings"),
                (0x8004e, "Food Buildings"),
                (0x8004f, "Military Buildings"),
                (0x80050, "Misc. Buildings"),
            ]);
            for slot in 1..=12u32 {
                let Some(t) = build_slot_type(st.category, slot, st.race) else { continue };
                let k = ((slot - 1) / 2) as u8;
                let (c0, c1) = if slot % 2 == 1 { (1, 19) } else { (21, 39) };
                v.push(b(c0, c1, 22 + 12 * k, 31 + 12 * k, Label::Building(t)));
            }
        }
        2 => quarters(&mut v, [
            (0x80051, "Inventory"),
            (0x80052, "Tools and Weapons"),
            (0x80081, "Priority of Buildings"),
            (0x80082, "Priority of Goods"),
        ]),
        3 => quarters(&mut v, [
            (0x8005a, "Population"),
            (0x8005b, "Occupations"),
            (0x8005c, "Recruitment"),
            (0x80155, "Production statistics"),
        ]),
        _ => {}
    }
    v
}

/// Squared distance from a point to a rectangle (0 inside).
fn dist2(r: (i32, i32, i32, i32), x: f32, y: f32) -> f32 {
    let dx = (r.0 as f32 - x).max(0.0).max(x - (r.0 + r.2) as f32);
    let dy = (r.1 as f32 - y).max(0.0).max(y - (r.1 + r.3) as f32);
    dx * dx + dy * dy
}

/// The button a finger at (x, y) means: the button under it, or (when `snap` and the page is fully
/// known) the nearest button within `snap_px`. Returns its index in `buttons`.
pub fn pick(l: &Layout, buttons: &[Button], x: f32, y: f32, snap: bool, snap_px: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, btn) in buttons.iter().enumerate() {
        let d = dist2(l.rect(btn), x, y);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    let (i, d) = best?;
    let limit = if snap { snap_px * snap_px } else { 0.0 };
    (d <= limit).then_some(i)
}

/// Centre of a button in screen pixels (where the click is sent).
pub fn center(l: &Layout, btn: &Button) -> (i32, i32) {
    let (x, y, w, h) = l.rect(btn);
    (x + w / 2, y + h / 2)
}

/// Decode game text bytes (Windows-1252 for the Western versions; HYPOTHESIS for other languages).
pub fn decode_cp1252(bytes: &[u8]) -> String {
    const HI: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’', '“',
        '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&c| match c {
            0x80..=0x9f => HI[(c - 0x80) as usize],
            c => c as char,
        })
        .collect()
}

/// Resolve a text id like `0x5faaa0` does, by reading the string table at `strtab` (`[gfx+0x5f6b8]`).
pub fn read_text(mem: &GuestMemory, strtab: u32, id: u32) -> Option<String> {
    if strtab < 0x1_0000 {
        return None;
    }
    let lang = mem.read_u32(strtab + 0xc40);
    let (t, ix) = (id >> 16, id & 0xffff);
    if lang > 16 || t >= 64 {
        return None;
    }
    let hdr = mem.read_u32(strtab + 0x37c + t * 4);
    let base = mem.read_u32(strtab + 0x264 + t * 4);
    if hdr < 0x1_0000 || base < 0x1_0000 {
        return None;
    }
    let n = mem.read_u16(hdr + 8) as u32;
    if ix >= n {
        return None;
    }
    let off = mem.read_u32(hdr + 0xc + (n * lang + ix) * 4);
    let a = base.wrapping_add(off);
    if a < 0x1_0000 {
        return None;
    }
    let len = mem.strlen(a, 160);
    let s = decode_cp1252(&mem.read_bytes(a, len));
    let s = s.trim().to_string();
    (!s.is_empty() && s.chars().all(|c| !c.is_control())).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const L640: Layout = Layout::builtin(0);

    #[test]
    fn grid_rects_match_recipes() {
        // integration-log recipe (640x480): menu icon at 21,452.
        let bs = buttons(&PanelState { page: 0, ..Default::default() });
        let menu = &bs[0];
        let (x, y, w, h) = L640.rect(menu);
        assert_eq!((x, y, w, h), (8, 443, 24, 21));
        assert!(x <= 21 && 21 < x + w && y <= 452 && 452 < y + h);
        assert_eq!(pick(&L640, &bs, 21.0, 452.0, false, 0.0), Some(0));
    }

    #[test]
    fn build_list_per_race() {
        let st = PanelState { page: 1, category: 1, race: 0, ..Default::default() };
        let bs = buttons(&st);
        let slots: Vec<_> = bs.iter().filter_map(|b| if let Label::Building(t) = b.label { Some(t) } else { None }).collect();
        // Romans: 11 entries in category 1 (slot 12 empty).
        assert_eq!(slots.len(), 11);
        assert_eq!(slots[0], 0x02);
        let asians = buttons(&PanelState { race: 2, ..st });
        assert_eq!(asians.iter().filter(|b| matches!(b.label, Label::Building(_))).count(), 12);
        assert_eq!(build_slot_type(4, 9, 3), Some(0x35));
        assert_eq!(build_slot_type(4, 9, 0), None);
        assert_eq!(building_text_id(0x0f), 0x8000f);
        assert_eq!(building_text_id(0x33), 0x8015f);
    }

    #[test]
    fn snapping_only_near_and_on_known_pages() {
        let st = PanelState { page: 1, category: 1, race: 0, ..Default::default() };
        let bs = buttons(&st);
        // Slot 1 is cols 1..19 rows 22..31 -> x 11..71, y 218..248; slot 3 starts at row 34 (y 254).
        let i = pick(&L640, &bs, 30.0, 250.0, true, 9.0).unwrap();
        assert!(matches!(bs[i].label, Label::Building(0x02)));
        let i = pick(&L640, &bs, 30.0, 253.0, true, 9.0).unwrap();
        assert!(matches!(bs[i].label, Label::Building(0x03)));
        // Column gap between the two slots (col 20 = x 68..70) snaps to a neighbour.
        assert!(pick(&L640, &bs, 69.5, 230.0, true, 9.0).is_some());
        // Far away (minimap area / map): nothing.
        assert_eq!(pick(&L640, &bs, 300.0, 230.0, true, 9.0), None);
        // Without snapping a gap is not a hit.
        assert_eq!(pick(&L640, &bs, 30.0, 252.5, false, 9.0), None);
        // Page 5 is not fully known.
        assert!(!PanelState { page: 5, ..Default::default() }.fully_known());
        assert!(st.fully_known());
    }

    #[test]
    fn layout_and_panel_area() {
        let l = Layout::builtin(1);
        assert!(l.in_panel(20.0, 300.0));
        assert!(!l.in_panel(20.0, 100.0)); // minimap
        assert!(!l.in_panel(200.0, 300.0)); // map
        let bs = buttons(&PanelState::default());
        // Same grid at 800x600: menu icon centre.
        assert_eq!(center(&l, &bs[0]), (8 + 16, 171 + 97 * 4 + 14));
    }

    #[test]
    fn cp1252() {
        assert_eq!(decode_cp1252(b"Caf\xe9 \x80"), "Café €");
    }
}
