#!/usr/bin/env python3
"""Analyse a 16-bit PCM WAV written by `s3run --audio-wav`: per-second RMS (dBFS, L/R), peak, clipped samples,
silent stretches and sudden drop-outs (signal -> exact zero -> signal, typical of mixer underruns).

usage: python3 -I tools/wav_stats.py file.wav [--from S] [--to S] [--window-ms 1000]
No third-party modules (wave + array only)."""
import array
import math
import sys
import wave


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        return 2
    path = args[0]
    t_from, t_to, win_ms = 0.0, None, 1000
    i = 1
    while i < len(args):
        if args[i] == "--from":
            t_from = float(args[i + 1]); i += 2
        elif args[i] == "--to":
            t_to = float(args[i + 1]); i += 2
        elif args[i] == "--window-ms":
            win_ms = int(args[i + 1]); i += 2
        else:
            print("unknown arg", args[i]); return 2
    w = wave.open(path, "rb")
    ch, sw, rate, n = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()
    if sw != 2:
        print("only 16-bit PCM supported"); return 2
    data = array.array("h")
    data.frombytes(w.readframes(n))
    if sys.byteorder != "little":
        data.byteswap()
    n = len(data) // ch
    print(f"{path}: {rate} Hz, {ch} ch, {n} frames = {n / rate:.2f} s")
    start = int(t_from * rate)
    end = n if t_to is None else min(n, int(t_to * rate))
    win = max(1, rate * win_ms // 1000)
    total_clip = 0
    silent_windows = 0
    print(" time     rmsL    rmsR   peak  clip  zero%  note")
    for ws in range(start, end, win):
        we = min(end, ws + win)
        sums = [0.0] * ch
        peak = 0
        clip = 0
        zeros = 0
        for f in range(ws, we):
            allz = True
            for c in range(ch):
                v = data[f * ch + c]
                sums[c] += v * v
                a = abs(v)
                if a > peak:
                    peak = a
                if a >= 32767:
                    clip += 1
                if v != 0:
                    allz = False
            if allz:
                zeros += 1
        cnt = we - ws
        db = [10 * math.log10(s / cnt) - 20 * math.log10(32768) if s > 0 else -120.0 for s in sums]
        total_clip += clip
        note = ""
        if peak == 0:
            note = "SILENT"
            silent_windows += 1
        elif zeros > cnt * 0.05 and zeros < cnt:
            note = "partly zero"
        line = f"{ws / rate:6.1f}s " + " ".join(f"{d:7.1f}" for d in db[:2]) + f" {peak:6d} {clip:5d} {100 * zeros / cnt:5.1f}  {note}"
        print(line)
    # Drop-outs: runs of >= 2 ms of exact zero frames between non-zero audio.
    min_run = rate // 500
    runs = []
    run_start = None
    seen_signal = False
    for f in range(start, end):
        z = all(data[f * ch + c] == 0 for c in range(ch))
        if z:
            if run_start is None:
                run_start = f
        else:
            if run_start is not None and seen_signal and f - run_start >= min_run:
                runs.append((run_start, f - run_start))
            run_start = None
            seen_signal = True
    short = [r for r in runs if r[1] < rate // 5]
    print(f"clipped samples: {total_clip}; silent windows: {silent_windows}; "
          f"zero gaps inside signal (2-200 ms): {len(short)}; longer silences: {len(runs) - len(short)}")
    for s, l in short[:20]:
        print(f"  gap at {s / rate:.3f}s len {1000 * l / rate:.1f} ms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
