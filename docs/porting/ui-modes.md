# UI modes compared: A "Classic Touch" vs B "Mobile HUD"

For the user: the two touch UIs side by side, so you can pick one (decision request from
[mobile-vision.md](mobile-vision.md)). Details: mode A [ui-classic.md](ui-classic.md), mode B [ui-hud.md](ui-hud.md),
shared map touch [touch.md](touch.md), framework [mods.md](mods.md). State: 2026-11-06, after ux-verify1.

Screenshots are **local only** (`scratch/` is gitignored, they contain game graphics). Paths are relative to the repo
root. Both modes run the unmodified GOG `S3.EXE`; everything is done by in-memory hooks (GOG sha256 `00a925cf…`
only; with another exe both modes stay off and the plain touch-to-mouse fallback is used).

## Shared by both modes (touch-context, W3)

Map gestures are the same in A and B, so switching modes does not change how you play on the map:

| Gesture | Effect |
| --- | --- |
| Tap own unit / building | select (enlarged hit box) |
| Tap ground with units selected | move / attack (a quick right click; **no long press anywhere**) |
| Tap a marker in build mode | place the construction site |
| ✕ (A) / Cancel banner (B), two-finger tap | cancel placement / deselect |
| One-finger drag, two-finger drag | pan the camera, with inertia |
| Long press, then drag | box select |
| Pinch | resolution step 640x480 <-> 800x600 <-> 1024x768 (the game's F1-F3) |

## Side by side

| | **A "Classic Touch"** | **B "Mobile HUD"** |
| --- | --- | --- |
| Look | The original game screen: left panel with minimap, tabs and build icons; a floating **Menu** button bottom right; a context chip top left ("Place: Woodcutter's Hut", "14 selected") | Full-screen map (the original panel is removed by a layout patch); the game's minimap top left; resource strip top right (Boards, Stone, Logs, Food, Pop); bottom bar Basic / Food / Military / Special / Settlers / Stats / Menu |
| Building | Tap the panel tab and icon. While the finger is down a 3x **magnifier bubble** shows the icon and the game's own name; lifting snaps the click to the button centre | Bottom **sheet with large cards** (icon + name, only your race's buildings), then a banner "Tap the map to place" with Cancel |
| Selected units / building | Original panel pages (units, building page) | Native **cards**: "14 selected: 10 x Swordsmen …" with Move to / Deselect; building card with Details / Destroy (tap twice) |
| Economy pages (goods, settlers, priorities, statistics) | Original panel pages, all reachable | Native goods/settlers summary sheets; deeper pages open the original page in a temporary "panel peek" with "Back to HUD" |
| Save / Quit / Pause | Menu -> original Save/Quit page | HUD menu: Resume, Save (panel peek), Pause, Quit, UI mode, full-screen on/off, HUD size -/+ |
| Map area at 640x480 | 504 px wide (panel takes ~21 %) | full 640 px |
| Touch targets | Panel icons are small (about 2.2x on a 1080p phone); the magnifier and snapping compensate | Bar buttons ~86x46, cards ~146x56 game px (about 2.25x on a 1080p phone) |
| Text / language | Game's own strings | Game's own strings (names read from the game's tables) |
| How much depends on our RE | Little: the original panel does the work; only the build pages and menu page are modelled for snapping | More: goods ids (some HYPOTHESIS), settler type names (partly "Type N"), stock summed from piles, layout patch of the landscape buffer |
| Switch to the other mode | Menu -> "UI: Mobile HUD" | Menu -> UI mode |

Screenshots (desktop `s3run --inject`, 640x480 unless noted):

| What | A | B |
| --- | --- | --- |
| In game | `scratch/ui-modes/A-ingame.png` | `scratch/ui-modes/B-01-ingame.png` |
| Choosing a building | `scratch/ui-modes/A-build-loupe.png` (magnifier bubble) | `scratch/ui-modes/B-02-build-basic.png`, `B-03-build-military.png` |
| Placement mode | `scratch/ui-modes/A-build-placing.png`, `A-cancelled.png` | `scratch/ui-modes/B-04-placement.png` |
| Units selected | `scratch/ui-modes/A-units.png`, `A-units-loupe.png` | `scratch/ui-modes/B-05-unit-card.png` |
| Building selected | (original building page) | `scratch/ui-modes/B-10-building-card.png` |
| Economy | (original pages) | `scratch/ui-modes/B-06-settlers.png`, `B-07-stats.png` |
| Game menu | `scratch/ui-modes/A-menu-open.png`, `A-menu-loupe.png`, `A-menu-closed.png` | `scratch/ui-modes/B-08-menu.png`, `B-09-save-peek.png` |
| 800x600 | (pinch / F2) | `scratch/ui-modes/B-11-800x600-ingame.png`, `B-12-800x600-build.png` |
| Full flow (box, move, pan, build, save, quit, load) | `scratch/ux-verify1/A/01..16-*.png` | `scratch/ux-verify1/B/00..17-*.png`, `B2/`, `B3/` |
| Mode switch | `scratch/ux-verify1/A2/02-menu.png` .. `04-hud.png` (A -> B) | `scratch/ux-verify1/B2/09-switchA.png`, `10-modeA.png` (B -> A) |

Android emulator shots of both modes (taps, also in a multiplayer game): `scratch/android-shots/ux1-*.png`
(list in [integration-log.md](integration-log.md), "Mobile UX verification 1").

## Pros and cons

**A "Classic Touch"**

- + Looks and plays exactly like the original; every panel page and feature works, because the game draws and
  handles it. Lowest risk of game-logic surprises.
- + Little RE needed, so it works the same for every race, page and future content.
- − Small icons; the magnifier helps, but browsing pages is slower than with big cards.
- − The panel takes a fifth of the screen width; on a phone the map is smaller.
- − Unmapped pages (units, building, messages, save slots) have no snapping and no names in the bubble.

**B "Mobile HUD"**

- + Big targets, full-screen map, feels like a mobile game (JSettlers-like); quickest to build and read stock.
- + HUD size scales (-/+), works at 800x600 too.
- − Covers only what we reverse-engineered; deeper pages fall back to the original panel ("panel peek").
- − Some data is HYPOTHESIS (a few good ids and settler names); the panel's own goods counters are not found, so
  stock is our sum over piles on your land.
- − Known glitches: runtime F1-F3 / pinch in HUD mode leaves a black band (start at 800x600 instead); after an
  in-game switch from A it covers the panel until the next game start; west map edge shows the void.

## Recommendation

Keep **A "Classic Touch" as the default** (it matches your stated preference to play on the original panel and is
the most complete and robust), and offer **B as a switch** in the in-game menu for players on small phones who want
the full map. Decide finally after a **real phone test**: if the panel icons are too small to read at 2.2x even with
the magnifier, make B the default for screens under ~6", or add the planned "enlarged panel" rendering to A.
