# Progress: audio-verify — FINAL (2026-11-05)

Goal: s3run `--audio-wav <file>` sink (mixed Host audio -> WAV, bounded), optional cpal playback feature,
verify music (MCI ogg) + effects + mixer timing via WAV analysis; fix mixer issues; document.

## Done
- `port/crates/s3-desktop/src/audio.rs`: WavWriter (bounded by `--audio-wav-seconds`, default 300; header patched
  every second), Timing (lead/drift/gaps vs wall clock, printed at exit), AudioSink, optional cpal 0.15 Player
  (cargo feature `audio-cpal`, default off; `--audio-play`). Wired into main.rs (Host::queue_audio,
  audio_queued_frames). 3 unit tests.
- Bug fixed: winmm `music_service` queued music to Host::queue_audio itself next to the DirectSound mixer (two
  producers). Now pushes into `AudioStreams[MUSIC_STREAM_ID]` (winmm.rs, LEAD_MS 300), flushed on stop/close/play.
  `S3_MUSIC_VOLUME` env knob.
- s3-dx dsound.rs: host-queue drift guard (`MAX_HOST_QUEUE_MS` 250), Debug log + counter per Play.
  3 new tests in s3-dx/src/tests.rs (passthrough/gapless stream, 22k resampling + pan, drift guard).
- tools/wav_stats.py (python3 -I): RMS per window, clipping, silence, zero gaps.
- Runs: in game reached (screenshot scratch/audio/final.png); music continuous -31/-28 dBFS, effects -35..-55 dBFS,
  no clipping, no gaps, lead flat 60 ms, drift 0 over 40 s.
- Docs: directx-hle.md (Audio verification section), user32-gdi32.md (music output), architecture.md,
  integration-log.md, status.md.

## Not done / open
- Real playback (`--audio-play`) only checked to open/run on an ALSA null device (no sound played on the user's
  machine on purpose). Listening test pending.

## Verify
cd port && CARGO_TARGET_DIR=$PWD/target cargo test -p s3-dx -p s3-win32 -p s3-desktop -j 4
cargo build -p s3-desktop --features audio-cpal -j 4
./target/release/s3run --headless ... --audio-wav ../scratch/audio/x.wav ; python3 -I ../tools/wav_stats.py ../scratch/audio/x.wav
