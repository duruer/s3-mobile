# Registry the game uses

Source: GOG `regs.cmd` (writes 32-bit view, HKLM). Our port must provide a fake registry pre-filled with these.

`HKLM\SOFTWARE\Bluebyte\Siedler3\1.0\General`:

| Value | Type | GOG default | Note |
| --- | --- | --- | --- |
| SingleNumber | SZ | long 4x repeated key | `SerialNumber` in the multiplayer exe ([exe-variants](../game-files/exe-variants.md)) |
| Resolution | DWORD | 0 | in-game mode, clamped 0..2: **0 = 640x480, 1 = 800x600, 2 = 1024x768**, always 16 bpp (static `0x405c80`, read at `0x40deb8`). Menus use 800x600 |
| NoAlpha | DWORD | 0 | |
| FogSpeed | DWORD | 1 | |
| GDIMouse | DWORD | 1 | GDI cursor instead of software cursor |
| WaitVBlank | DWORD | 1 | HYPOTHESIS: settings `+0x5C` → `WaitForVerticalBlank` before each Blt (`0x42cddb`) |
| ScrollMode / ScrollSpeed | DWORD | 0 / 5 | |
| SoundFormat | DWORD | 1 | |
| CDAudio | DWORD | 2 | |
| VideoFormat | DWORD | 1 | |
| MessageLevel | DWORD | 9 | |
| Intro | DWORD | 1 | |
| Language | DWORD | 9 | 9 = English (LANGID primary) |
| Tips&Tricks, Plus, Gold, Mission, BuildHelp, FirstLogon | DWORD | 1 | expansions enabled |
| ClanFlag, Newbie, BannerId, BannerSubId | DWORD | 0 | |
| Font / FontSize | SZ / DWORD | "MS Sans Serif" / 15 | GDI font → we need a font substitute |
| Campaigns / CampaignsPlus | BINARY | 0x34-byte blobs | campaign progress (encrypted?) |
| Playername | SZ | "Player" | |
| GameZoneSelection | DWORD | 511 | |

Other keys: `...\Siedler3\1.0\Patches` (`convert`=0), `HKLM\SOFTWARE\Bluebyte\S3Editor` (editor paths),
`HKLM\SOFTWARE\Bluebyte\S3PLUSCD`, `App Paths\s3.exe`. Exe also references `\Index Server`, `NoFlip`, `\Patches`.
