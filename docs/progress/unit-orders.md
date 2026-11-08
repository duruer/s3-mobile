# Progress: unit-orders (waypoints / patrol / paths / control groups)

Worktree: .claude/worktrees/agent-a8cc234c3d1474648, branch worktree-agent-a8cc234c3d1474648 (recreated 2026-11-08 after
a stall; the earlier run left nothing). Runtime scratch: scratch/unit-orders/ (main checkout, gitignored).

## Goal
1. Modifier-click orders (Shift/Ctrl/Alt + right click with selection) as touch buttons next to "Move to".
2. Path display (dotted) for selected units + setting "Show unit paths".
3. Control-group bar 1..6 (setting "Group shortcuts").

## Done
- RE static (scratch/re-ui/s3.asm, manual MANUAL/in_keyb/keyb.htm + military/military.htm read as data):
  - HandleInput key switch: `vk-9` -> byte table 0x534cbc -> jump table 0x534c00. Digits: Alt -> 0x542820(n) assign,
    else 0x542660(n) select (2nd press while slot==n: centre camera via gd::CenterOn 0x51b560). Tab 0x525788 = keep
    wounded. Groups = UI selection slots 1..9, slot 0 = plain selection.
  - Move executor 0x536045: 0xd Shift -> mode 1 waypoint, 0xe Alt -> 2 patrol, 0xf Ctrl -> 3 ignore enemies.
- RE runtime VERIFIED (s3run r2/r3/r4): Alt+1 assigns (count slot1 = 9), key 1 selects slot 1, 2nd press moves camera.
  Shift+RMB: waypoints at settler +0x30/+0x32, +0x34/+0x36 (+0x38/+0x3a, 0xffff empty). Alt+RMB patrol:
  +0x30 = start, +0x34 = target, +0x38 = 0xfffe/0xfffe, job +0x0e = 0x35 (walking by command; 0x37 idle).
  Map->screen exact: sx = 16x - 8y - [gd+0x2d254] + 8W + 280 + 32*res, sy = 9y - h - [gd+0x2d258] + 182
  (h = tile+4 byte; res = [0x730f08]); dx 0 / dy+h 182 for all 14 units.
- s3-desktop inject: `kdown VK` / `kup VK`.

- unit_orders.rs written (mod + HandleInput hook), fallback right_next_vk, lib set_order_next/order_next,
  settings show_unit_paths/group_shortcuts, HUD card order row + settings rows 6/7, Classic float toggles
  (2-column overflow). Classic run c2 OK (paths, waypoint, group assign). Fix: deselect no longer clears a group
  slot (touch_game::deselect, HUD ClearSelection -> slot -1).

- Classic c3 (2316x904) + HUD h1/h2 (2176x1812) screenshots OK; cargo test -p s3-mods 92 passed; docs + journal.

## In progress
- (done) APK builds; committed.

## Open
- Many units of a big group draw many parallel dotted lines (busy, but readable).
- Ctrl (no fight) effect not traced in the executor (manual only).

## Verify
- cargo test -p s3-mods; sh port/scripts/build-apk.sh; recipes in docs/porting/touch.md "Unit orders, paths, groups".
