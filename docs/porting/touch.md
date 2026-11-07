# Context-aware touch (W3)

Status: implemented 2026-11-05 (agent `touch-context`). Shared by both UI modes (A "Classic Touch", B "Mobile HUD",
[mobile-vision.md](mobile-vision.md)). Framework: [mods.md](mods.md). Game facts used:
[game-state-units.md](../reverse-engineering/game-state-units.md), [game-state-ui.md](../reverse-engineering/game-state-ui.md).

Code (`port/crates/s3-mods/src/`):

| File | What |
| --- | --- |
| `touch.rs` | the `touch-context` mod: gestures -> game actions, camera pan/inertia, pinch zoom, feedback state; `install` (hook on `HandleInput` `0x524fa0` = "a game is running") |
| `touch_logic.rs` | pure decision logic + camera physics (unit tested): `Layout::region`, `find_hit`, `decide_tap`, `decide_double_tap`, `decide_two_finger_tap`, `drag_kind`, `pinch_zoom_key`, `clamp_camera`, `CameraPan` |
| `touch_game.rs` | GOG S3.EXE memory reads/writes (addresses below) -> `View` snapshot |
| `touch_fx.rs` | overlay drawing: ripple, move/attack flag, selection rings, long-press ring, selection box, ✕ cancel button |

Frontends deliver raw pointers only (`HostEvent::Pointer`): Android `s3-android/src/touch.rs` (MotionEvent ->
pointer events, see [android.md](android.md)) and desktop `s3run --inject` touch commands
(`s3-desktop/src/touch_inject.rs`). The gesture recognizer and everything below run in `s3-mods` on the engine
thread, so desktop tests exercise exactly the Android code path.

## Behaviour

| Gesture | In game | Outside a game (menus) |
| --- | --- | --- |
| Tap own unit / building (map) | select: left click at the unit (enlarged hit box, see below) | – |
| Tap ground, units selected | **move**: quick right click (down, up 60 ms later) at the point = the game's own command 9 path (incl. selection sync cmd 3, sound, minimap targets). Green flag on the map | left click |
| Tap enemy unit/building, units selected | **attack**: quick right click on the enemy. Red flag | – |
| Tap ground, nothing selected | left click (the game's default: deselect / nothing) | – |
| Tap in build placement | left click = place the site (the game checks the spot) | – |
| Tap ✕ (top right of the map, placement only) / two-finger tap | cancel placement (`[gd+0x2d28c] = 0`, panel dirty) | – |
| Tap panel / minimap | left click (panel buttons, minimap jump; choosing another building or tab also ends placement) | left click |
| Double tap own soldier | Ctrl + left click = select the same type nearby (game rule, `0x533f21`) HYPOTHESIS: Ctrl reaches the game through our WM_KEYDOWN; not verified at runtime | – |
| Two-finger tap | placement: cancel; building selected: quick right click (deselect); units selected: clear the selection; else nothing | right click (fallback) |
| Three-finger tap | soft keyboard request (fallback) | same |
| One-finger drag on the map | **camera follows the finger** (direct camera write), inertia after a fling | left drag (fallback) |
| One-finger drag on panel / minimap | left drag | – |
| Two-finger drag | camera follows the centroid, inertia | – |
| Long press | hover only (mouse move = tooltips after ~10 logic ticks). A white ring grows from 150 ms and stays full while held | hover |
| Long press, then drag | **box selection** (left drag from the press point); green box drawn by us, the game draws its own | left drag |
| Pinch in / out | **zoom** = render resolution levels (res-zoom, [resolution-zoom.md](resolution-zoom.md)): continuous, one level per finger-distance factor 1.12, the map point under the fingers stays; levels 360..780 px high at the window aspect. With the custom resolution off: the old F1/F2/F3 step (640x480 <-> 800x600 <-> 1024x768, threshold scale 0.75 / 1.33) | – |

**No long press is ever a right click.** The original game ignores right-ups held for 15 or more game steps
(`0x53437d`), so every synthetic right click is a quick down/up. (The `RMB` letterbox button was removed 2026-11-07; if
"next tap = right click" is armed via `s3_mods::set_right_next`, `touch-context` passes the tap to the fallback.

Hit test: the game's own single-click box is x ±10 px, feet `y-5 ..= y+32` (`0x5339a1`). We look for hit-list
entries in a larger box (x ±18, feet `y-12 ..= y+44`), prefer the local player's own entries, then the nearest to the
body, and click at `(feet.x, feet.y - 12)`, which lies inside the game's box. So taps near a unit select it.

Feedback (overlay, map effects clipped to the map view): white ripple on every in-game tap (orange for cancel,
green/red for move/attack), a flag with a pulsing ground ring at move/attack targets that stays anchored to the
map while panning (`world = screen + camera`), green rings under every selected own unit that is on screen (hit list
∩ UI selection), the long-press ring, the selection box, and the ✕ "Cancel" button in placement mode.

## Game state used (GOG `00a925cf…` only)

| What | Address | Status |
| --- | --- | --- |
| `gd` (CGameData) | `[0x7e3408]`, valid when `[gd] == 0x658c18`; **in multiplayer `[0x7e3408]` stays 0**, then `[logic+0x3a0]` (ECX of `HandleInput`, recorded by `touch_game::note_logic`) | VERIFIED (0 in menus before the first game, `0x18790000` in game; MP: 0 on host and joiner, fallback works, ux-verify1) |
| Game running | hook `CLogicThread::HandleInput` `0x524fa0`: called every logic tick; "in game" = a call within 1.5 s **and** the layout globals match the presented frame size | VERIFIED in game (taps in the menus still went to the fallback, so it is not called there; HYPOTHESIS for the statistics screen) |
| Layout | `0x730f08` res index, `0x730f0c/10` screen, `0x730f18/1c` map view, `0x730f24` panel width, `0x730f2c` minimap x, `0x730f34` minimap bottom | VERIFIED (re-ui) |
| Mode | page `[gd+0x2d284]` (1 build, 4 building, 9 paused), slot `[gd+0x2d28c]`, building `[0x7dfd6c]`, UI selection slot `[0x7dfd68]` + count `word [0x7ad620+slot*2]` | VERIFIED |
| Hit list | `0x7ade08 + ([0x7dfd64] + 2i) * 10`, count `[0x7dfd5c]`; entry `u16 kind, i16 x, i16 y, u16 id` | VERIFIED (2026-11-05: kind 100 tower id 1 at (332,222); kind 3 id 0x35 at (212,316)) |
| Owner | settler `gd+0x12a5e60 + id*0x40` +0, building `gd+0x1499e64 + id*0x3a` +0, local player `[gd+0x2d2d4]` | VERIFIED (re-units) |
| Camera | write `gd+0x2d25c` / `+0x2d260`, clamp in host with the formula of `0x51b4d0` | VERIFIED (drag 100 px -> camera +100 exactly) |
| Cancel placement | `[gd+0x2d28c] = 0`, `[gd+0x2d27c] = 1` | VERIFIED (slot 1 -> 0, ✕ and two-finger tap) |
| Deselect units | count `:= 0`, dirty `[0x7ad5f8+slot*4] = 1`, page 5 -> 0, panel dirty | VERIFIED (count 10 -> 0, panel cleared, the game's own selection brackets gone). The logic-side copy is re-synced by the next right click (cmd 3 with count 0) |

Why a synthetic right click and not posting command 9 directly: the right-click path is VERIFIED end to end
(selection sync cmd 3, acknowledgement sound, minimap target, modifiers, attack on enemies) while direct
`Queue::Post` of commands is still a HYPOTHESIS (must run on the logic thread). The click is mouse input, so it also
goes through the lockstep command queue in multiplayer like any player action.

## Verified on desktop (2026-11-05, `scratch/touch-context/r*.png`, local only)

Recipe (in game after ~45-50 s on a loaded machine; random map of the first single-player game):

```sh
D=../scratch/touch-context; G=0x7e3408
I="1200:tap 165 235;3200:tap 630 467;5200:tap 730 567;7200:tap 730 567;\
50000:lpdrag 300 330 470 470 700 400;51450:shot $D/r1box.png;52500:tap 540 160;52750:shot $D/r2flag.png;\
56000:tap2 400 300;56300:peek 0x7ad620 4;57000:drag2 400 300 250 200;57320:xpeek $G@0x2d25c 8;\
59000:drag 300 300 400 300 1000;60100:xpeek $G@0x2d25c 8;61000:tap 25 167;61800:tap 23 194;62600:tap 38 230;\
63400:shot $D/r5build.png;64000:tap 612 28;64300:xpeek $G@0x2d284 12;65000:tap 38 230;66000:tap2 400 300;\
67000:longpress 400 300 1000;67700:shot $D/r7press.png;68500:pinch 400 300 0.6;71000:peek 0x730f08 4"
./target/release/s3run --headless --game-dir ../game-files/gog-s3-ultimate --data-dir $D/data --max-seconds 76 --inject "$I"
```

Results: long-press drag selected 10-11 soldiers (green rings + the game's brackets); tap on the ground made them
walk there (flag); two-finger tap cleared the selection; two-finger drag of 150 px moved the camera +159 px (then
glided ~110 px); a slow 100 px drag moved it exactly 100 px with no glide; build slot chosen by taps -> ✕ visible ->
✕ cancelled (slot 1 -> 0), and so did a two-finger tap; long-press ring drawn; pinch 0.6 switched 640x480 -> 800x600
and pinch 1.6 switched back.

Not verified at runtime: attack on an enemy (no enemy in reach on the test map; logic unit-tested), double tap = same
type selection, tap on a building with units selected.

## Known limits / notes

- **Resolution switch cool-down**: a second pinch ~3.5 s (wall time, slow host) after a switch was ignored by the
  game; 7 s later it worked. HYPOTHESIS: the game ignores F1-F3 until its display re-init finished. We fire the key
  once per gesture; the user pinches again.
- The camera clamp is the game's rectangle, so flings can reach the black corners of the diamond-shaped world, like
  the game's own scrolling.
- Fling: release speed measured over the last 80 ms; glides only above 0.25 game px/ms, time constant 300 ms,
  max 4 px/ms. Any touch-down stops a glide; the game moving the camera (minimap, keys) also stops it.
- The ✕ sits at the top right of the map view (`touch_logic::cancel_button`); UI mode A moved its Menu button to the
  bottom right for that reason. UI-mode mods are added **after** `touch-context` in `s3_mods::install`, so they are
  on top and see touches first.
- Settings: `touch.context` (default `true`) in `mods-settings.json`; `false` = legacy touch->mouse fallback only
  (which still turns long presses into right clicks if `long_press_right_click`). Unknown exes and `S3_MODS=0` also
  fall back to the legacy mapping.
- `S3_TOUCH_TRACE=1` prints camera writes and pinch decisions to stderr.
- Old Android letterbox buttons (ESC, ABC, RMB) removed 2026-11-07. Back sends Esc; three-finger tap toggles the
  keyboard.

## Tests

`cargo test -p s3-mods touch` (16): regions incl. the sheared minimap, enlarged hit box clicking inside the game box,
own-before-enemy, move/attack/select/panel taps, placement place/✕/two-finger cancel, two-finger tap per mode,
double tap, drag kinds, pinch steps, camera clamp, follow + glide + stop, two-finger equal-timestamp samples,
no glide after a resting finger or a slow drag. `s3-desktop`: `inject_touch_parsing` (lpdrag/drag2/tap3/dtap).
`s3-android`: `motion_actions_map_to_pointer_phases`, `filter_drops_unchanged_moves`.
