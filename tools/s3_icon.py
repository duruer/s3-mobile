#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Extract the game's own icon from the user's local S3.EXE (PE RT_ICON resources) or a .ICO file
and write Android launcher PNGs (drawable-*dpi/ic_launcher.png) into an output res directory.

The output is build-time only (port/target/...), never committed: the icon is game artwork.
Pure stdlib (struct + zlib). Run with `python -I tools/s3_icon.py <S3.EXE|x.ico> <out-res-dir>`
or `--list <file>` to print the available icon images.
"""
import struct
import sys
import zlib
from pathlib import Path

DENSITIES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}


def pe_icons(data):
    """Yield (id, bytes) for every RT_ICON (type 3) resource in a PE image."""
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    nsec = struct.unpack_from("<H", data, pe + 6)[0]
    optsz = struct.unpack_from("<H", data, pe + 20)[0]
    opt = pe + 24
    rsrc_rva = struct.unpack_from("<I", data, opt + 96 + 2 * 8)[0]
    secs = []
    for i in range(nsec):
        s = pe + 24 + optsz + 40 * i
        vsz, va, rsz, rptr = struct.unpack_from("<IIII", data, s + 8)
        secs.append((va, max(vsz, rsz), rptr))

    def off(rva):
        for va, sz, rptr in secs:
            if va <= rva < va + sz:
                return rva - va + rptr
        raise ValueError(f"rva {rva:#x} not in a section")

    base = off(rsrc_rva)

    def entries(o):
        nn, ni = struct.unpack_from("<HH", data, o + 12)
        for i in range(nn + ni):
            name, child = struct.unpack_from("<II", data, o + 16 + 8 * i)
            yield name, child

    for name, child in entries(base):
        if name != 3 or not child & 0x80000000:
            continue
        for iid, c2 in entries(base + (child & 0x7FFFFFFF)):
            for _lang, c3 in entries(base + (c2 & 0x7FFFFFFF)):
                rva, size = struct.unpack_from("<II", data, base + c3)
                yield iid, data[off(rva):off(rva) + size]


def ico_icons(data):
    n = struct.unpack_from("<H", data, 4)[0]
    for i in range(n):
        sz, o = struct.unpack_from("<II", data, 6 + 16 * i + 8)
        yield i, data[o:o + sz]


def decode(img):
    """Icon image (DIB or PNG) -> (w, h, rgba rows) ; PNG images are returned as raw bytes."""
    if img[:4] == b"\x89PNG":
        return None
    hsz, w, h2, _planes, bpp = struct.unpack_from("<IiiHH", img, 0)
    h = h2 // 2
    ncol = struct.unpack_from("<I", img, 32)[0] or (1 << bpp if bpp <= 8 else 0)
    pal = [img[hsz + 4 * i:hsz + 4 * i + 4] for i in range(ncol)]
    p = hsz + 4 * ncol
    stride = ((w * bpp + 31) // 32) * 4
    mstride = ((w + 31) // 32) * 4
    mask = p + stride * h
    rows = []
    for y in range(h):
        sy = h - 1 - y
        row = bytearray()
        for x in range(w):
            if bpp == 32:
                b, g, r, a = img[p + sy * stride + 4 * x:p + sy * stride + 4 * x + 4]
            else:
                if bpp == 24:
                    b, g, r = img[p + sy * stride + 3 * x:p + sy * stride + 3 * x + 3]
                else:
                    bit = x * bpp
                    v = img[p + sy * stride + bit // 8]
                    v = (v >> (8 - bpp - bit % 8)) & ((1 << bpp) - 1)
                    b, g, r = pal[v][:3]
                m = (img[mask + sy * mstride + x // 8] >> (7 - x % 8)) & 1
                a = 0 if m else 255
            row += bytes((r, g, b, a))
        rows.append(bytes(row))
    return w, h, rows, bpp


def png(w, h, rows):
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + r for r in rows)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def scale(w, h, rows, size):
    """Nearest-neighbour upscale (pixel art stays crisp), centred on a size x size canvas."""
    k = max(1, size // max(w, h))
    out_w = w * k
    pad = (size - out_w) // 2
    blank = bytes(4 * size)
    out = []
    for y in range(size):
        sy = (y - pad) // k
        if y < pad or sy >= h:
            out.append(blank)
            continue
        src = rows[sy]
        line = bytearray(4 * pad)
        for x in range(w):
            line += src[4 * x:4 * x + 4] * k
        line += bytes(4 * (size - pad - out_w))
        out.append(bytes(line))
    return out


def best(path):
    data = Path(path).read_bytes()
    it = pe_icons(data) if data[:2] == b"MZ" else ico_icons(data)
    imgs = []
    for iid, img in it:
        d = decode(img)
        if d:
            imgs.append((iid, *d))
    return imgs


def main(argv):
    if len(argv) == 3 and argv[1] == "--list":
        for iid, w, h, _rows, bpp in best(argv[2]):
            print(f"id {iid}: {w}x{h} {bpp} bpp")
        return 0
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    imgs = best(argv[1])
    if not imgs:
        print("no icon found", file=sys.stderr)
        return 1
    # Largest, then deepest colour.
    _iid, w, h, rows, _bpp = max(imgs, key=lambda t: (t[1] * t[2], t[4]))
    out = Path(argv[2])
    for dpi, size in DENSITIES.items():
        d = out / f"drawable-{dpi}"
        d.mkdir(parents=True, exist_ok=True)
        (d / "ic_launcher.png").write_bytes(png(size, size, scale(w, h, rows, size)))
    print(f"icon {w}x{h} -> {out}/drawable-*/ic_launcher.png")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
