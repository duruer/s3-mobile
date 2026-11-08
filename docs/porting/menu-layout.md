# Full-screen menu layouts (menu-fill, 2026-11-07)

User request: the pre-game menus must fill the phone screen (no black bars), cropping and re-arranging as needed
("not everything has to stay"). Related: [display.md](display.md) (display modes, the shared `Viewport`),
[android.md](android.md).

## Problem

S3.EXE draws every menu as a fixed **800x600** (4:3) frame. The test phone (Galaxy Z Fold4 cover screen) is
2316x904 (~2.56:1): `Fit` leaves ~640 px black bars, plain `Fill` keeps only 312 of the 600 rows and cuts buttons
(main menu buttons span y 18..574). So each known screen gets its own **layout**.

## Design (`port/crates/s3-mods/src/menu_layout.rs`)

- A `MenuLayout` = canvas size (game-pixel units) + background + `pieces` (source rect of the frame -> position on
  the canvas, same size) + `neutral` game point + `probes`.
- The canvas is fitted into the window (one uniform scale); the background (plain colour, or a stretched source
  rect) covers the whole window first -> no bars. Plain colours are used on purpose: the game draws the **mouse
  cursor into the frame**, so a tiled / stretched part of the frame repeated the cursor all over the window.
- **Detection** (`detect`): each probe is the expected 4x4 block mean (8-bit RGB, tolerance 14 per channel) at a
  static spot. All but one probe must match (the cursor can cover one). Probes avoid buttons (hover / pressed
  states) and include spots that the screen's popups cover, so a popup over a menu -> no layout -> normal `Fit`
  (never unusable). Only 800x600 frames, only in display modes `NativeWide` (default) and `Fill`.
- **Viewport**: `Viewport::for_frame(mode, pixels, w, h, win_w, win_h)` returns a viewport with
  `layout: Some(..)`; `window_to_game` maps through the pieces (outside every piece -> `neutral`, a spot without
  buttons), `game_to_window` maps a game pixel to its piece (not shown -> far off screen), `bars()` is zero,
  `visible_game_rect` is the whole frame. Touch input therefore needs no change.
- **Rendering**: Android GPU presenter (`present.rs`): `glClearColor` = background colour, then one quad per piece
  (vertex shader `u_uv` sub-rectangle, works with the sharp-bilinear shader). CPU (`s3run` window, previews):
  `render_xrgb` (nearest).
- Presentation only: guest memory is never touched (lockstep MP unaffected).

## Responsive: whole screen when it fits (2026-11-08)

The layouts drop decoration (helmets, logos, version) so the buttons are big on a ~2.56:1 phone. On other
shapes that is not needed. Every layout has a whole-frame variant (`<name>-full`, `menu_layout::FULL_LAYOUTS`:
the 800x600 frame 1:1, the bars filled with the screen's background colour, same detection). For every frame the
viewport picks with the current window size (`menu_layout::choose`): the whole frame when it can be shown at
least `FULL_PREFER` = 0.8 times as big as the re-arranged layout, else the layout. So it follows folding,
split screen and resizing at once.

| Window | Shape | Main menu |
| --- | --- | --- |
| Fold4 cover 2316x904 | 2.56:1 | layout (2.69 vs 1.51 px per game px) |
| 20:9 phone 2400x1080 | 2.22:1 | layout |
| 16:9 1920x1080 | 1.78:1 | whole menu (1.80 vs 2.23) |
| 16:10 tablet 2560x1600 | 1.6:1 | whole menu |
| 4:3 tablet 2048x1536 | 1.33:1 | whole menu |
| Fold4 inner 2176x1812 | 1.2:1 | whole menu (VERIFIED on the phone) |

`s3run --layout-preview frame.png out.png WxH` shows the choice for a window size. Tested by
`menu_layout::tests::responsive_choice`.

## Screens (GOG S3.EXE 1.60)

| Layout | Screen | What it does |
| --- | --- | --- |
| `main-menu` | main menu | 13 buttons in two columns (7 + 6, original gaps kept), laurel logo right; helmets, Blue Byte logo, version text dropped. Popups ("Campaign: Normal" info, "Internet connection not detected") -> Fit |
| `choose-race` | Campaign -> Choose a Race | title + the three race cards side by side |
| `choose-race-mcd` | Mission CD Campaign | band y 60..372 of the painting, title / Egyptians / Romans labels moved into it |
| `choose-race-amazon` | Amazon Campaign | band y 90..402, title / Amazons / Others moved into it |
| `map-picker` | Single Player: Scenario (map popup over Game Setup) | tabs + list, options/preview, Cancel/OK |
| `game-setup` | Game Setup | blocks in three columns |
| `players` | player list / MP waiting room | race icons + map info top, list (first ~9 rows), status + Cancel/OK bottom |
| `lan-game` | Multi-player Game: LAN | lists left, title / status / Back / Join / New in a right column. Popup "Before connecting, a provider has to be selected" -> Fit |
| `load-game` | Load Game | upper 330 rows of the list (~14 entries), buttons right |
| `briefing` | campaign mission briefing | text panel left, map right, tip text + Cancel/OK below |
| `loading` | 640x480 loading screen before a map (laurel + helmets) | kept whole, sides filled with its beige |

Window bands above / below a canvas (tall windows, e.g. Fold4 inner 2176x1812 with `main-menu-full`) are filled
with the background colour; `MenuLayout::band_tiles` repeats edge strips there (`menu_layout::band_quads`, used by
the CPU renderer and the GLES presenter). The main menu's Greek-key borders repeat every 62 rows, so they now run
along the whole window height (user report 2026-11-08; checked with `--layout-preview` at 2176x1812, 2316x904,
2048x1536).

Not covered (shown as before): Credits (scrolling text), popups, any screen not
listed. Lists are cut at the bottom (Load Game, players): long lists need the game's own scrolling.

## Tools

```sh
# screenshots (local only: game graphics, never commit): scratch/keep/menus/
s3run --headless --game-dir ../game-files/gog-s3-ultimate --data-dir ../scratch/d --max-seconds 9 \
  --inject "1000:move 165 355;1200:click 165 355;4000:shot ../scratch/keep/menus/load.png"
s3run --layout-preview frame.png out.png 2316x904 [layout-name]   # render through the detected (or named) layout
s3run --layout-probe frame.png 390,40 400,300                       # print pr(..) lines for new probes
```

Add a screen: take a shot, measure rectangles (any image viewer), add a `MenuLayout` + push it to `LAYOUTS`, pick
4-6 probes (static, no buttons, at least one under the screen's popups), check every shot in
`scratch/keep/menus` with `--layout-preview` (it prints the detected layout), run
`cargo test -p s3-mods --lib menu_layout`.

## Verified

- 2026-11-07 desktop previews of all screens at 2316x904; all 32 shots detect the right layout, popup shots -> Fit.
- 2026-11-07 phone (Fold4 cover, 2316x904): main menu full screen; tap on the moved "Load Game" (window 884,235 ->
  game 166,355) opened Load Game, shown with its layout. Tutorial loading screen (`loading`) full screen.
