#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Render our own app icon (port/android/res/drawable/ic_launcher_{bg,fg}.xml: roman numeral III, white with a red
outline on navy) as the 1024x1024 PNG iOS wants (port/ios/Resources/AppIcon.png, `iconPath` in xtool.yml).

Usage: python3 tools/make_ios_icon.py [out.png]
"""
import math
import re
import sys

from PIL import Image, ImageDraw, PngImagePlugin  # noqa: F401 (registers PNG)

SIZE = 1024
SS = 4  # supersampling
# The adaptive icon's visible square is the inner 72 of 108 units (18..90).
VIEW0, VIEW = 18.0, 72.0

# Paths of the three bars (left, right, middle) from ic_launcher_fg.xml, with their gradient y range.
BARS = [
    ("M30.00,36.00H45.00V39.20Q41.00,39.20 41.00,42.08V63.92Q41.00,66.80 45.00,66.80V70.00H30.00V66.80Q34.00,66.80 34.00,63.92V42.08Q34.00,39.20 30.00,39.20Z", 36, 70),
    ("M63.00,36.00H78.00V39.20Q74.00,39.20 74.00,42.08V63.92Q74.00,66.80 78.00,66.80V70.00H63.00V66.80Q67.00,66.80 67.00,63.92V42.08Q67.00,39.20 63.00,39.20Z", 36, 70),
    ("M45.50,30.00H62.50V33.60Q58.00,33.60 58.00,36.84V69.16Q58.00,72.40 62.50,72.40V76.00H45.50V72.40Q50.00,72.40 50.00,69.16V36.84Q50.00,33.60 45.50,33.60Z", 30, 76),
]
RED = (0x84, 0x2B, 0x26)
GREY = (0x8A, 0x8F, 0x96)


def path_points(d):
    """M/H/V/Q/Z path (absolute) -> polygon points in path units."""
    toks = re.findall(r"[MHVQZ]|-?\d+(?:\.\d+)?", d)
    pts, cur, i = [], (0.0, 0.0), 0
    while i < len(toks):
        c = toks[i]
        i += 1
        if c == "M":
            cur = (float(toks[i]), float(toks[i + 1]))
            i += 2
            pts.append(cur)
        elif c == "H":
            cur = (float(toks[i]), cur[1])
            i += 1
            pts.append(cur)
        elif c == "V":
            cur = (cur[0], float(toks[i]))
            i += 1
            pts.append(cur)
        elif c == "Q":
            c1 = (float(toks[i]), float(toks[i + 1]))
            end = (float(toks[i + 2]), float(toks[i + 3]))
            i += 4
            for k in range(1, 13):
                t = k / 12
                x = (1 - t) ** 2 * cur[0] + 2 * (1 - t) * t * c1[0] + t * t * end[0]
                y = (1 - t) ** 2 * cur[1] + 2 * (1 - t) * t * c1[1] + t * t * end[1]
                pts.append((x, y))
            cur = end
        elif c == "Z":
            pass
    return pts


def to_px(p, dx=0.0, dy=0.0):
    s = SIZE * SS / VIEW
    return ((p[0] + dx - VIEW0) * s, (p[1] + dy - VIEW0) * s)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "port/ios/Resources/AppIcon.png"
    n = SIZE * SS
    # Background: radial gradient #24466F (centre 54,48) -> #0C1B30 at radius 70.
    bg = Image.new("RGB", (n, n))
    px = bg.load()
    s = n / VIEW
    for y in range(0, n, SS):
        for x in range(0, n, SS):
            ux, uy = x / s + VIEW0, y / s + VIEW0
            t = min(1.0, math.hypot(ux - 54, uy - 48) / 70)
            col = tuple(round(a + (b - a) * t) for a, b in zip((0x24, 0x46, 0x6F), (0x0C, 0x1B, 0x30)))
            for yy in range(SS):
                for xx in range(SS):
                    px[x + xx, y + yy] = col
    img = bg.convert("RGBA")
    d = ImageDraw.Draw(img)
    stroke = round(4.2 * s)
    for path, _, _ in BARS:
        poly = [to_px(p) for p in path_points(path)]
        # Red outline (stroke 4.2, round joins approximated by the filled polygon + thick line).
        d.polygon(poly, fill=RED)
        d.line(poly + [poly[0]], fill=RED, width=stroke, joint="curve")
        for q in poly:
            r = stroke / 2
            d.ellipse([q[0] - r, q[1] - r, q[0] + r, q[1] + r], fill=RED)
        d.polygon(poly, fill=GREY)
    for path, y0, y1 in BARS:
        poly = [to_px(p, -0.6, -0.6) for p in path_points(path)]
        mask = Image.new("L", (n, n), 0)
        ImageDraw.Draw(mask).polygon(poly, fill=255)
        grad = Image.new("RGBA", (n, n))
        gd = ImageDraw.Draw(grad)
        top, bot = to_px((0, y0 - 0.6))[1], to_px((0, y1 - 0.6))[1]
        for y in range(max(0, int(top)), min(n, int(bot) + 1)):
            t = (y - top) / max(1.0, bot - top)
            c = tuple(round(255 + (b - 255) * t) for b in (0xC9, 0xCD, 0xD3))
            gd.line([(0, y), (n, y)], fill=c + (255,))
        img.paste(grad, (0, 0), mask)
    img = img.convert("RGB").resize((SIZE, SIZE), Image.LANCZOS)
    img.save(out, format="PNG")
    print(f"wrote {out} {img.size}")


if __name__ == "__main__":
    main()
