//! Mobile HUD: static tables of the GOG S3.EXE build menu (game-state-ui.md §2, STATIC from `0x526f8a..0x528a37`)
//! and short labels. No game data is copied here; only ids.

/// The four build categories of the original panel (cat 1..4) with our short HUD labels.
pub const CATEGORIES: [(&str, &str); 4] =
    [("Basic", "Basic buildings"), ("Food", "Food buildings"), ("Military", "Military buildings"), ("Special", "Special buildings")];

/// Building type for `(race, cat 1..4, slot 1..12)`, or `None` if the slot is empty for that race.
/// Races (HYPOTHESIS from the race-specific drinks): 0 Romans, 1 Egyptians, 2 Asians, 3 Amazons.
pub fn build_type(race: u32, cat: u32, slot: u32) -> Option<u8> {
    let r = race.min(3) as usize;
    let t: u8 = match (cat, slot) {
        (1, 1) => 0x02,
        (1, 2) => 0x04,
        (1, 3) => 0x03,
        (1, 4) => 0x05,
        (1, 5) => 0x09,
        (1, 6) => 0x0b,
        (1, 7) => 0x08,
        (1, 8) => 0x0a,
        (1, 9) => 0x07,
        (1, 10) => 0x0c,
        (1, 11) => [0x27, 0x25, 0x1d, 0x38][r],
        (1, 12) => [0, 0, 0x28, 0x25][r],
        (2, 1) => 0x19,
        (2, 2) => 0x18,
        (2, 3) => 0x17,
        (2, 4) => 0x11,
        (2, 5) => 0x15,
        (2, 6) => 0x14,
        (2, 7) => 0x1e,
        (2, 8) => 0x31,
        (2, 9) => [0x0e, 0x26, 0x16, 0x33][r],
        (2, 10) => [0, 0, 0x24, 0x34][r],
        (3, 1) => 0x0f,
        (3, 2) => 0x10,
        (3, 3) => 0x12,
        (3, 4) => 0x06,
        (3, 5) => 0x0d,
        (3, 6) => [0x1f, 0x2f, 0x30, 0x32][r],
        (3, 7) => 0x13,
        (3, 8) => 0x20,
        (3, 9) => 0x23,
        (4, 1) => 0x1a,
        (4, 2) => 0x1b,
        (4, 3) => 0x1c,
        (4, 4) => 0x01,
        (4, 5) => 0x22,
        (4, 6) => 0x21,
        (4, 7) => [0x2c, 0x2a, 0x2e, 0x36][r],
        (4, 8) => [0x2b, 0x29, 0x2d, 0x37][r],
        (4, 9) => [0, 0, 0, 0x35][r],
        _ => 0,
    };
    (t != 0).then_some(t)
}

/// Text id of a building type's name (table 8). Types >= 0x33 are Amazon buildings with their own ids.
pub fn building_text_id(t: u8) -> u32 {
    match t {
        0x33 => 0x8015f,
        0x34 => 0x80160,
        0x35 => 0x80161,
        0x32 => 0x8015e,
        0x36 => 0x8002c,
        0x37 => 0x8002b,
        0x38 => 0x8001d,
        t => 0x80000 + t as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_table_matches_doc() {
        assert_eq!(build_type(0, 1, 1), Some(0x02));
        assert_eq!(build_type(0, 1, 12), None);
        assert_eq!(build_type(2, 1, 12), Some(0x28));
        assert_eq!(build_type(3, 4, 9), Some(0x35));
        assert_eq!(build_type(0, 4, 9), None);
        assert_eq!(build_type(1, 2, 9), Some(0x26));
        assert_eq!(build_type(0, 3, 10), None);
        // Every category has a first slot for every race.
        for r in 0..4 {
            for c in 1..=4 {
                assert!(build_type(r, c, 1).is_some());
            }
        }
        assert_eq!(building_text_id(0x0f), 0x8000f);
        assert_eq!(building_text_id(0x33), 0x8015f);
    }
}

/// Good ids seen in the pile table (`gd+0x123885c`, byte +0) matched to the original Inventory page
/// (goods page, tab 1) by their amounts on random-map starts (2026-11-05, docs/porting/ui-hud.md):
/// `(good id, inventory row, inventory column, English label)`. Ids 1/2 (Boards/Stone, both 25 at start) and
/// 17/32 (Scythe/Fishing rod, both 1) could not be told apart by amount: HYPOTHESIS (ordered like the panel).
/// Logs (3) is a HYPOTHESIS from the gap in the id sequence.
pub const GOODS: [(u8, u8, u8, &str); 15] = [
    (1, 0, 0, "Boards"),
    (2, 0, 1, "Stone"),
    (3, 0, 2, "Logs"),
    (5, 0, 3, "Coal"),
    (7, 0, 4, "Iron ore"),
    (13, 1, 1, "Hammer"),
    (12, 1, 2, "Shovel"),
    (14, 1, 3, "Axe"),
    (16, 1, 4, "Saw"),
    (15, 1, 5, "Pickaxe"),
    (17, 2, 0, "Scythe"),
    (32, 2, 1, "Fishing rod"),
    (8, 3, 0, "Fish"),
    (24, 3, 2, "Meat"),
    (4, 3, 5, "Bread"),
];

/// Label of a good id.
pub fn good_name(g: u8) -> Option<&'static str> {
    GOODS.iter().find(|e| e.0 == g).map(|e| e.3)
}

/// Pixel rectangle `(x, y, w, h)` of the Inventory page icon at `(row, col)`. Measured at 640x480 (grid cell 3,
/// origin 8,152): x = 14 + 18*col, y = 247 + 27*row, 16x16; other resolutions scale by the cell size
/// (HYPOTHESIS, the panel is defined in grid cells).
pub fn inventory_icon_rect(row: u8, col: u8, cell: i32, gx: i32, gy: i32) -> (i32, i32, i32, i32) {
    let s = |v: i32| v * cell / 3;
    (gx + s(6 + 18 * col as i32), gy + s(95 + 27 * row as i32), s(16), s(16))
}
