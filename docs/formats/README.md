# Data formats

Since we run the original exe, **we do not need to parse these to play**. We need them only for: launcher
validation (is this a valid install?), previews (map thumbnails in the launcher), debugging.

Status legend: VERIFIED (checked on our files) / COMMUNITY (from JSettlers or others, not yet checked here).

| Format | Files | Status | Notes |
| --- | --- | --- | --- |
| Graphics | `GFX/siedler3_NN.<masks>.dat` | partial | Two copies per index: `7c003e01f` = RGB555, `f8007e01f` = RGB565 (VERIFIED by name). Game picks set by DirectDraw pixel format (COMMUNITY). Contains settler/building/landscape/GUI image sequences + palettes (COMMUNITY, JSettlers `jsettlers.graphics` reader). |
| Sound | `SND/Siedler3_00.dat`, `siedler3_01.dat` | COMMUNITY | PCM sample archive (JSettlers reads it). |
| Music | `MUSIC/TrackNN.ogg` | VERIFIED | Ogg Vorbis, track N = former CD audio track N. |
| Maps | `*.map` / `*.MAP` | COMMUNITY | Chunked, obfuscated/compressed (JSettlers has a reader). Filename prefix convention `<size>-<players>-<name>` for multiplayer. |
| Saves | `Save/**` | unknown | |
| Campaign | `S3*/MIS_V|MIS_M|ADD/*.MAP .DAT .WAV` | partial | WAV = mission voice-over (VERIFIED by extension). |

References: JSettlers (open-source Java remake) has readers for GFX/SND/MAP — use only as format documentation.
