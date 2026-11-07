# menu-fill (2026-11-07)

Goal (user): pre-game screens (800x600 menus) full screen on the phone (2316x904, ~2.56:1) without black bars:
crop (Fill), and where needed re-arrange parts of the frame per screen. Not everything must stay visible.

Plan:
1. Tour every pre-game screen with desktop `s3run --inject ... t:shot` (800x600 frames) -> scratch/keep/menus/.
2. Per-screen "layouts": list of src rect (game px) -> dst rect (window, normalised) + inverse touch mapping,
   presentation only (no guest memory change). Screen detection by pixel signature or guest state.
3. Presenter draws several quads; touch maps via the same rects.

Done:
- Step 1: screenshots of all main-menu targets in scratch/keep/menus/*.png (s3run, 800x600).
- `port/crates/s3-mods/src/menu_layout.rs`: types (MenuLayout/Piece/Probe/Background), detect, canvas mapping,
  CPU renderer, MAIN_MENU layout (probes still empty = never matches). Registered in lib.rs.

- display.rs: Viewport.layout + for_frame/with_layout, mapping, scale_to_xrgb -> menu_layout::render_xrgb. DONE.
- 10 layouts with probes (main-menu, 3x choose-race, map-picker, game-setup, players, lan-game, load-game,
  briefing); all scratch/keep/menus screenshots detect correctly, popups -> None (Fit). Plain colour backgrounds
  (cursor is in the frame, tiles repeated it).
- s3run --layout-preview / --layout-probe (s3-desktop/src/layout_tool.rs). DONE.

- GPU path (gl.rs u_uv, present.rs pieces), desktop uses for_frame. Unit tests (4). docs/porting/menu-layout.md.
- Phone: main menu full screen OK; Load Game via moved button OK.

- Bugs fixed + phone-verified: loading layout; camera y clamp patch 0x51b528 (patchset.rs gog_base).

- Start camera centred (res.rs switch_hook default anchor, ui_hud_game shift_camera_x). Phone: Scenario flow through
  map-picker / game-setup / players layouts into the game OK.

In progress: none. Untested on phone: race screens, LAN, briefing.
Next: Scenario -> map picker -> setup -> players; Campaign race -> briefing; LAN; adjust pieces if needed.
Phone: adb 192.168.1.52:34223 (port changes).
