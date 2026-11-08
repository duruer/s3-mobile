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

// ---------------------------------------------------------------- settler types

/// Where the HUD takes the icon of a settler type from (all sampled from the running game, never stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlerIcon {
    /// Icon `i` (0..12, row-major 4 per row) of the original settlers page, tab "Population" (page 3 tab 1).
    Pop(u8),
    /// The build-menu icon of this building type (the profession's workplace).
    Building(u8),
    /// The race's siege workshop (catapult / ballista / cannon hall, Amazon gong hall).
    WarMachine,
}

/// Icons of the Population tab (renderer `0x4b7e96..0x4b8833`), row-major: carriers, diggers, builders,
/// workers / swordsmen, bowmen, spearmen, priests / geologists, thieves, pioneers, donkeys.
pub const POP_ICONS: u8 = 12;

/// Pixel rectangle `(x, y, w, h)` of Population icon `i` (grid cells: cols `1 + 10*(i%4)`, rows `41 + 18*(i/4)`,
/// 8x12 cells; the counts are drawn below at rows 55 / 73 / 91, `0x4b8713..`). VERIFIED at 640x480 (cell 3,
/// origin 8,152: icons at x 11 / 41 / 71 / 101, y 275 / 329 / 383, 24x36).
pub fn pop_icon_rect(i: u8, cell: i32, gx: i32, gy: i32) -> (i32, i32, i32, i32) {
    let (col, row) = ((i % 4) as i32, (i / 4) as i32);
    (gx + (1 + 10 * col) * cell, gy + (41 + 18 * row) * cell, 8 * cell, 12 * cell)
}

/// What the HUD knows about a settler type (settler entry byte +1, game-state-units.md "Settler types").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlerInfo {
    pub name: String,
    pub icon: SettlerIcon,
    /// Part of the game's settler total (Population tab, `0x5216ff..0x521936`): false for ships, war machines,
    /// the tower figure and donkeys (the tab shows a donkey count, but outside the total).
    pub counted: bool,
    /// Sort key of the settlers sheet (the Population tab's order: carriers, diggers, builders, soldiers,
    /// specialists, then professions).
    pub order: u8,
    /// Shown in the settlers sheet (false for the tower figure, type 0x25).
    pub listed: bool,
}

/// Name, icon and grouping of settler type `t` for race `race` (0 Romans, 1 Egyptians, 2 Asians, 3 Amazons).
/// Mapping RE'd 2026-11-08 (hud-settlers): Population tab counting code (`0x5216ff..`), selection kinds
/// (`0x542a83..`), the building worker requests (switch `0x4e0e1a` over the building type, each case calls
/// `0x4dacd0(building, worker type)`) and runtime checks (workers in the AI's buildings). `None` = unknown id.
pub fn settler_info(t: u8, race: u32) -> Option<SettlerInfo> {
    use SettlerIcon::*;
    let amazon = race == 3;
    let soldier = |kind: usize, level: u8| -> (String, SettlerIcon, u8) {
        let names = if amazon { ["Swordswoman", "Bowwoman", "Spearwoman"] } else { ["Swordsman", "Bowman", "Spearman"] };
        (format!("{} L{level}", names[kind]), Pop(4 + kind as u8), 10 + kind as u8 * 3 + level)
    };
    // (name, icon, order) and counted / listed flags.
    let (name, icon, order): (String, SettlerIcon, u8) = match t {
        0x00 => ("Carrier".into(), Pop(0), 0),
        0x01 => ("Digger".into(), Pop(1), 1),
        0x02 => ("Builder".into(), Pop(2), 2),
        0x05 => soldier(0, 1),
        0x20 => soldier(0, 2),
        0x23 => soldier(0, 3),
        0x08 => soldier(1, 1),
        0x1e => soldier(1, 2),
        0x21 => soldier(1, 3),
        0x0f => soldier(2, 1),
        0x1f => soldier(2, 2),
        0x22 => soldier(2, 3),
        0x24 => ("Priest".into(), Pop(7), 25),
        0x16 => ("Geologist".into(), Pop(8), 26),
        0x17 => ("Thief".into(), Pop(9), 27),
        0x28 => ("Pioneer".into(), Pop(10), 28),
        0x27 => ("Donkey".into(), Pop(11), 29),
        // Professions: worker type requested by the building type (switch 0x4e0e1a).
        0x03 => ("Woodcutter".into(), Building(0x02), 40),
        0x04 => ("Stonecutter".into(), Building(0x03), 41),
        0x06 => ("Sawmill worker".into(), Building(0x04), 42),
        0x07 => ("Forester".into(), Building(0x05), 43),
        // Coal / gold / iron ore / sulfur / gem mines (0x07, 0x08, 0x09, 0x1d, 0x25, 0x38).
        0x0a => ("Miner".into(), Building(0x07), 44),
        // Gold and iron smelter (0x0a, 0x0b).
        0x09 => ("Smelter".into(), Building(0x0b), 45),
        // Tool smith, weapon smith and the siege workshops (0x0c, 0x0d, 0x1f, 0x2f, 0x30, 0x32).
        0x0b => ("Smith".into(), Building(0x0c), 46),
        0x18 => ("Charcoal maker".into(), Building(0x27), 47),
        0x1b => ("Gunpowder maker".into(), Building(0x28), 48),
        0x11 => ("Fisherman".into(), Building(0x19), 50),
        0x10 => ("Grain farmer".into(), Building(0x18), 51),
        // Pig farm and donkey farm (0x17, 0x31).
        0x1c => ("Animal farmer".into(), Building(0x17), 52),
        0x0c => ("Miller".into(), Building(0x11), 53),
        0x0e => ("Butcher".into(), Building(0x15), 54),
        0x0d => ("Baker".into(), Building(0x14), 55),
        0x12 => ("Waterworker".into(), Building(0x1e), 56),
        0x1d => ("Winegrower".into(), Building(0x0e), 57),
        0x1a => ("Brewer".into(), Building(0x26), 58),
        0x19 => ("Distiller".into(), Building(0x16), 59),
        0x26 => ("Rice farmer".into(), Building(0x24), 60),
        0x2d => ("Beekeeper".into(), Building(0x33), 61),
        0x2e => ("Mead maker".into(), Building(0x34), 62),
        0x2f => ("Alchemist".into(), Building(0x35), 63),
        0x13 => ("Shipwright".into(), Building(0x20), 64),
        0x15 => ("Healer".into(), Building(0x23), 65),
        // Counted as a worker by the Population tab, but no building requests it (HYPOTHESIS: unused).
        0x14 => ("Worker".into(), Pop(3), 66),
        // Not settlers in the Population tab: selection kinds 4 / 3 / 2 (`0x542aa5`, `0x542a94`, `0x542a83`).
        0x29 => {
            let n = ["Catapult", "Ballista", "Cannon", "War machine"][race.min(3) as usize];
            (n.into(), WarMachine, 80)
        }
        0x2b => ("Trade ship".into(), Building(0x20), 81),
        0x2c => ("Transport ship".into(), Building(0x20), 82),
        // One per military building, created next to it (`0x4dbf83`), job 0xad (HYPOTHESIS: the figure on the
        // tower). Not counted, not listed.
        0x25 => ("Tower guard".into(), Pop(4), 90),
        _ => return None,
    };
    let counted = !matches!(t, 0x25 | 0x27 | 0x29 | 0x2b | 0x2c);
    Some(SettlerInfo { name, icon, counted, order, listed: t != 0x25 })
}

/// The race's siege workshop building type (build menu cat 3 slot 6).
pub fn war_machine_building(race: u32) -> u8 {
    build_type(race, 3, 6).unwrap_or(0x1f)
}

/// Build-menu position `(cat, slot)` of building type `bt` for `race` (where its harvested icon is).
pub fn build_slot_of(race: u32, bt: u8) -> Option<(u32, u32)> {
    (1..=4u32).flat_map(|c| (1..=12u32).map(move |s| (c, s))).find(|&(c, s)| build_type(race, c, s) == Some(bt))
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

    #[test]
    fn settler_types_have_names() {
        // Every type the Population tab counts (0x5216ff..0x521844) has a real name for every race.
        let counted: [u8; 46] = [
            0, 1, 2, 0x24, 0x16, 0x17, 0x28, 0x27, 5, 0x20, 0x23, 0xf, 0x1f, 0x22, 8, 0x1e, 0x21, 3, 4, 6, 7, 9, 0xa,
            0xb, 0xc, 0xd, 0xe, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x26, 0x2d,
            0x2e, 0x2f, 0x29, 0x2b, 0x2c,
        ];
        for r in 0..4 {
            for &t in &counted {
                let i = settler_info(t, r).unwrap_or_else(|| panic!("type {t:#x}"));
                assert!(!i.name.starts_with("Type"), "{t:#x}");
                if let SettlerIcon::Building(b) = i.icon {
                    // Race-specific workplaces only need a slot for their race.
                    if ![0x0e, 0x16, 0x24, 0x26, 0x27, 0x28, 0x33, 0x34, 0x35].contains(&b) {
                        assert!(build_slot_of(r, b).is_some(), "type {t:#x} building {b:#x} race {r}");
                    }
                }
            }
        }
        assert_eq!(settler_info(0x20, 0).unwrap().name, "Swordsman L2");
        assert_eq!(settler_info(0x22, 3).unwrap().name, "Spearwoman L3");
        assert!(!settler_info(0x25, 0).unwrap().listed);
        assert!(!settler_info(0x2c, 0).unwrap().counted);
        assert_eq!(settler_info(0x2a, 0), None);
        assert_eq!(build_slot_of(0, 0x02), Some((1, 1)));
        assert_eq!(build_slot_of(3, 0x33), Some((2, 9)));
        assert_eq!(war_machine_building(2), 0x30);
        assert_eq!(pop_icon_rect(0, 3, 8, 152), (11, 275, 24, 36));
        assert_eq!(pop_icon_rect(11, 3, 8, 152), (101, 383, 24, 36));
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

// ---------------------------------------------------------------- building panel (page 4) buttons

/// Building types whose panel has "Stop / Resume Working" (command 0x04), from the type list at `0x531c72..0x531d1a`
/// (construction sites always have "Stop / Resume Building").
pub const STOP_WORK_TYPES: [u8; 36] = [
    0x02, 0x03, 0x04, 0x05, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x11, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19,
    0x1d, 0x1e, 0x1f, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2f, 0x30, 0x31, 0x13, 0x32, 0x33, 0x34, 0x35, 0x38,
];

/// Building types with "Set new working area" (`0x5320ac..0x5320d4`): woodcutter, stonecutter, forester,
/// fisherman, grain farm, rice farm, winery, waterworks, beekeeper.
pub const WORK_AREA_TYPES: [u8; 9] = [0x02, 0x1e, 0x0e, 0x19, 0x24, 0x18, 0x05, 0x03, 0x33];

/// Military buildings with garrison buttons (`0x531450`): guard tower, large guard tower, castle.
pub const GARRISON_TYPES: [u8; 3] = [0x0f, 0x10, 0x12];

/// Garrison actions of command 0x0c (`0x531499..0x531ab6`, byte +7) with their tooltip text ids.
pub const GARRISON_ACTIONS: [(u8, u32); 8] = [
    (2, 0x80073), // Add Swordsman
    (6, 0x80074), // Remove Swordsman
    (3, 0x80075), // Add Bowman
    (7, 0x80076), // Remove Bowman
    (4, 0x80077), // Add Spearman
    (8, 0x80078), // Remove Spearman
    (1, 0x80071), // Maximum Garrison
    (5, 0x80072), // Minimum Garrison
];

/// Settlers page "Recruitment" buttons: (command 0x10 kind, count, text id). Amazons (race 3) use the text ids
/// `+0xfe` (`0x80177..0x8017e`, same meaning).
pub const RECRUIT_ACTIONS: [(u8, u8, u32); 8] = [
    (1, 1, 0x8007a),
    (1, 5, 0x80079),
    (2, 1, 0x8007c),
    (2, 5, 0x8007b),
    (3, 1, 0x8007e),
    (3, 5, 0x8007d),
    (4, 1, 0x80080),
    (4, 5, 0x8007f),
];
