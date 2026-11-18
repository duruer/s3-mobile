"""Generate the README illustrations in docs/assets/readme/ (our own vector art, no game pixels).

Run: python3 -I tools/readme_art.py   (writes the SVG files; deterministic)
"""
import os
import random

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "docs", "assets", "readme")

NAVY = "#0c1424"
NAVY2 = "#17264a"
CARD = "#141f38"
LINE = "#2a3a63"
GOLD = "#f2c14e"
GOLD2 = "#e09a2b"
TEXT = "#eef2f8"
MUTED = "#9fb0d0"
RED = "#a8322d"
GREEN = "#3fb36b"
SANS = "Segoe UI, Helvetica Neue, Arial, sans-serif"
SERIF = "Georgia, Times New Roman, serif"


def svg(w, h, body, defs=""):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">'
        f"<defs>{defs}</defs>{body}</svg>\n"
    )


def write(name, text):
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, name), "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


BG_DEFS = f"""
<radialGradient id="bg" cx="70%" cy="35%" r="85%">
  <stop offset="0" stop-color="#22396b"/><stop offset="0.55" stop-color="{NAVY2}"/><stop offset="1" stop-color="{NAVY}"/>
</radialGradient>
<linearGradient id="gold" x1="0" y1="0" x2="0" y2="1">
  <stop offset="0" stop-color="#ffe08a"/><stop offset="1" stop-color="{GOLD2}"/>
</linearGradient>
<radialGradient id="glow" cx="50%" cy="50%" r="50%">
  <stop offset="0" stop-color="{GOLD}" stop-opacity="0.35"/><stop offset="1" stop-color="{GOLD}" stop-opacity="0"/>
</radialGradient>
<pattern id="hex" width="56" height="97" patternUnits="userSpaceOnUse">
  <path d="M28 0 L56 16 L56 48 L28 64 L0 48 L0 16 Z M28 64 L28 97" fill="none" stroke="#ffffff" stroke-opacity="0.04" stroke-width="1.5"/>
</pattern>
<linearGradient id="grass" x1="0" y1="0" x2="1" y2="1">
  <stop offset="0" stop-color="#78b84a"/><stop offset="1" stop-color="#4c8a32"/>
</linearGradient>
"""


def emblem(x, y, s):
    """Rounded navy square with the roman numeral III (same idea as the app icon)."""
    r = s * 0.22
    fs = s * 0.62
    return (
        f'<g transform="translate({x} {y})">'
        f'<rect width="{s}" height="{s}" rx="{r}" fill="#1e3a6e" stroke="#ffffff" stroke-opacity="0.12" stroke-width="2"/>'
        f'<text x="{s/2}" y="{s*0.72}" text-anchor="middle" font-family="{SERIF}" font-weight="bold" font-size="{fs}" '
        f'fill="#e9edf3" stroke="{RED}" stroke-width="{s*0.05}" paint-order="stroke" letter-spacing="{-s*0.02}">III</text>'
        "</g>"
    )


# ---------------------------------------------------------------- map scene


def tree(x, y, s, rnd):
    dark = rnd.choice(["#2d6a2a", "#2a5f27", "#357a30"])
    return (
        f'<rect x="{x-s*0.08:.1f}" y="{y:.1f}" width="{s*0.16:.1f}" height="{s*0.35:.1f}" fill="#6b4a2b"/>'
        f'<circle cx="{x:.1f}" cy="{y-s*0.15:.1f}" r="{s*0.42:.1f}" fill="{dark}"/>'
        f'<circle cx="{x-s*0.12:.1f}" cy="{y-s*0.3:.1f}" r="{s*0.18:.1f}" fill="#4f9a3c" opacity="0.8"/>'
    )


def house(x, y, s, roof="#b5452f"):
    return (
        f'<rect x="{x-s*0.5:.1f}" y="{y-s*0.55:.1f}" width="{s:.1f}" height="{s*0.6:.1f}" fill="#efe1bd" stroke="#6b4a2b" stroke-width="0.8"/>'
        f'<path d="M{x-s*0.62:.1f} {y-s*0.5:.1f} L{x:.1f} {y-s*1.05:.1f} L{x+s*0.62:.1f} {y-s*0.5:.1f} Z" fill="{roof}"/>'
        f'<rect x="{x-s*0.1:.1f}" y="{y-s*0.3:.1f}" width="{s*0.2:.1f}" height="{s*0.35:.1f}" fill="#6b4a2b"/>'
        f'<ellipse cx="{x:.1f}" cy="{y+s*0.08:.1f}" rx="{s*0.7:.1f}" ry="{s*0.12:.1f}" fill="#000" opacity="0.15"/>'
    )


def tower(x, y, s):
    return (
        f'<rect x="{x-s*0.3:.1f}" y="{y-s*1.3:.1f}" width="{s*0.6:.1f}" height="{s*1.3:.1f}" fill="#c9c3b5" stroke="#5c564c" stroke-width="0.8"/>'
        f'<path d="M{x-s*0.4:.1f} {y-s*1.3:.1f} h{s*0.8:.1f} v-{s*0.2:.1f} h-{s*0.2:.1f} v{s*0.1:.1f} h-{s*0.13:.1f} v-{s*0.1:.1f} h-{s*0.14:.1f} v{s*0.1:.1f} h-{s*0.13:.1f} v-{s*0.1:.1f} h-{s*0.2:.1f} Z" fill="#c9c3b5" stroke="#5c564c" stroke-width="0.8"/>'
        f'<path d="M{x:.1f} {y-s*1.5:.1f} v-{s*0.5:.1f} l{s*0.35:.1f} {s*0.12:.1f} l-{s*0.35:.1f} {s*0.12:.1f}" fill="#3b6fe0" stroke="#3b6fe0" stroke-width="1"/>'
    )


def map_scene(w, h, seed=3):
    """Original stylised landscape: meadow, river, hills, forest, village, fields."""
    rnd = random.Random(seed)
    p = [f'<rect width="{w}" height="{h}" fill="url(#grass)"/>']
    for _ in range(14):
        p.append(
            f'<ellipse cx="{rnd.uniform(0,w):.0f}" cy="{rnd.uniform(0,h):.0f}" rx="{rnd.uniform(w*0.08,w*0.2):.0f}" '
            f'ry="{rnd.uniform(h*0.05,h*0.14):.0f}" fill="{rnd.choice(["#8cc65a","#5a9a3a","#6aa845"])}" opacity="0.45"/>'
        )
    # hills / mountain top right
    mx = w * 0.78
    p.append(
        f'<path d="M{mx-w*0.25:.0f} {h*0.42:.0f} L{mx-w*0.08:.0f} {h*0.06:.0f} L{mx+w*0.02:.0f} {h*0.24:.0f} '
        f'L{mx+w*0.12:.0f} {-h*0.02:.0f} L{mx+w*0.3:.0f} {h*0.4:.0f} Z" fill="#8d8b86"/>'
        f'<path d="M{mx-w*0.08:.0f} {h*0.06:.0f} L{mx-w*0.03:.0f} {h*0.15:.0f} L{mx-w*0.13:.0f} {h*0.16:.0f} Z" fill="#f4f4f0"/>'
        f'<path d="M{mx+w*0.12:.0f} {-h*0.02:.0f} L{mx+w*0.18:.0f} {h*0.1:.0f} L{mx+w*0.06:.0f} {h*0.1:.0f} Z" fill="#f4f4f0"/>'
        f'<path d="M{mx-w*0.25:.0f} {h*0.42:.0f} L{mx-w*0.08:.0f} {h*0.06:.0f} L{mx-w*0.02:.0f} {h*0.42:.0f} Z" fill="#000" opacity="0.12"/>'
    )
    # river
    river = f"M{-10} {h*0.78:.0f} C{w*0.2:.0f} {h*0.62:.0f} {w*0.35:.0f} {h*0.98:.0f} {w*0.55:.0f} {h*0.8:.0f} S{w*0.8:.0f} {h*0.5:.0f} {w+10} {h*0.58:.0f}"
    p.append(f'<path d="{river}" fill="none" stroke="#2f6ea8" stroke-width="{h*0.075:.0f}" stroke-linecap="round"/>')
    p.append(f'<path d="{river}" fill="none" stroke="#5aa6e6" stroke-width="{h*0.04:.0f}" stroke-linecap="round"/>')
    p.append(f'<path d="{river}" fill="none" stroke="#bfe3ff" stroke-width="1.2" stroke-dasharray="6 14" opacity="0.8"/>')
    # fields
    for fx, fy in [(w * 0.08, h * 0.36), (w * 0.2, h * 0.44)]:
        fw, fh = w * 0.1, h * 0.12
        p.append(f'<rect x="{fx:.0f}" y="{fy:.0f}" width="{fw:.0f}" height="{fh:.0f}" fill="#e0c55e" stroke="#a88a2e" stroke-width="0.8"/>')
        for i in range(1, 6):
            p.append(f'<line x1="{fx:.0f}" y1="{fy+fh*i/6:.1f}" x2="{fx+fw:.0f}" y2="{fy+fh*i/6:.1f}" stroke="#b89a3a" stroke-width="0.8"/>')
    # roads
    road = f"M{w*0.1:.0f} {h*0.3:.0f} L{w*0.36:.0f} {h*0.4:.0f} L{w*0.5:.0f} {h*0.32:.0f} L{w*0.62:.0f} {h*0.5:.0f} M{w*0.36:.0f} {h*0.4:.0f} L{w*0.4:.0f} {h*0.62:.0f}"
    p.append(f'<path d="{road}" fill="none" stroke="#c9a86b" stroke-width="{max(2.5, h*0.014):.1f}" stroke-linecap="round" stroke-linejoin="round"/>')
    # territory border
    p.append(
        f'<path d="M{w*0.03:.0f} {h*0.12:.0f} Q{w*0.4:.0f} {-h*0.02:.0f} {w*0.66:.0f} {h*0.2:.0f} T{w*0.7:.0f} {h*0.7:.0f}" '
        f'fill="none" stroke="#3b6fe0" stroke-width="2" stroke-dasharray="2 5" stroke-linecap="round"/>'
    )
    # forests
    items = []
    for cx, cy, n in [(w * 0.62, h * 0.72, 9), (w * 0.9, h * 0.82, 8), (w * 0.12, h * 0.68, 6), (w * 0.52, h * 0.12, 6)]:
        for _ in range(n):
            items.append(("t", cx + rnd.uniform(-w * 0.07, w * 0.07), cy + rnd.uniform(-h * 0.08, h * 0.08)))
    s = h * 0.075
    for x, y in [(w * 0.3, h * 0.36), (w * 0.42, h * 0.3), (w * 0.36, h * 0.52), (w * 0.5, h * 0.44), (w * 0.24, h * 0.26)]:
        items.append(("h", x, y))
    items.append(("w", w * 0.58, h * 0.4))
    items.sort(key=lambda it: it[2])
    for kind, x, y in items:
        if kind == "t":
            p.append(tree(x, y, s, rnd))
        elif kind == "h":
            p.append(house(x, y, s * 1.1, rnd.choice(["#b5452f", "#9c3b2a", "#c0582f"])))
        else:
            p.append(tower(x, y, s * 1.1))
    # settlers walking on the road
    for t in [0.18, 0.27, 0.45, 0.55]:
        x = w * (0.1 + 0.26 * t / 0.55 * 1.3)
        y = h * (0.3 + 0.1 * t / 0.55 * 1.3)
        p.append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{h*0.012+1:.1f}" fill="#3b6fe0" stroke="#fff" stroke-width="0.8"/>')
    return "".join(p)


# ---------------------------------------------------------------- phone + HUD


def phone(x, y, w, h, inner, clip_id, rot=0):
    bez = h * 0.06
    sw, sh = w - 2 * bez, h - 2 * bez
    return (
        f'<g transform="translate({x} {y}) rotate({rot} {w/2} {h/2})">'
        f'<rect x="6" y="14" width="{w}" height="{h}" rx="{h*0.13:.0f}" fill="#000" opacity="0.35"/>'
        f'<rect width="{w}" height="{h}" rx="{h*0.13:.0f}" fill="#0a0d14" stroke="#3a4766" stroke-width="2"/>'
        f'<clipPath id="{clip_id}"><rect x="{bez}" y="{bez}" width="{sw}" height="{sh}" rx="{h*0.08:.0f}"/></clipPath>'
        f'<g clip-path="url(#{clip_id})"><g transform="translate({bez} {bez})">{inner(sw, sh)}</g></g>'
        f'<circle cx="{bez*0.5}" cy="{h/2}" r="{bez*0.18:.1f}" fill="#1c2433"/>'
        "</g>"
    )


def glyph(kind, cx, cy, s, col=TEXT):
    """Tiny line icons used in mock UIs and feature tiles."""
    if kind == "house":
        return f'<path d="M{cx-s*0.45} {cy+s*0.4} V{cy-s*0.05} L{cx} {cy-s*0.45} L{cx+s*0.45} {cy-s*0.05} V{cy+s*0.4} Z" fill="none" stroke="{col}" stroke-width="{s*0.11}" stroke-linejoin="round"/>'
    if kind == "wheat":
        return (f'<path d="M{cx} {cy+s*0.45} V{cy-s*0.45}" stroke="{col}" stroke-width="{s*0.1}"/>'
                + "".join(f'<ellipse cx="{cx+d*s*0.16}" cy="{cy-s*0.3+i*s*0.2}" rx="{s*0.1}" ry="{s*0.07}" fill="{col}" transform="rotate({d*35} {cx+d*s*0.16} {cy-s*0.3+i*s*0.2})"/>' for i in range(3) for d in (-1, 1)))
    if kind == "sword":
        return (f'<path d="M{cx-s*0.35} {cy+s*0.35} L{cx+s*0.4} {cy-s*0.4}" stroke="{col}" stroke-width="{s*0.12}" stroke-linecap="round"/>'
                f'<path d="M{cx-s*0.38} {cy+s*0.05} L{cx-s*0.05} {cy+s*0.38}" stroke="{col}" stroke-width="{s*0.12}" stroke-linecap="round"/>')
    if kind == "star":
        return f'<path d="M{cx} {cy-s*0.45} L{cx+s*0.13} {cy-s*0.13} L{cx+s*0.45} {cy-s*0.1} L{cx+s*0.2} {cy+s*0.12} L{cx+s*0.28} {cy+s*0.45} L{cx} {cy+s*0.27} L{cx-s*0.28} {cy+s*0.45} L{cx-s*0.2} {cy+s*0.12} L{cx-s*0.45} {cy-s*0.1} L{cx-s*0.13} {cy-s*0.13} Z" fill="none" stroke="{col}" stroke-width="{s*0.09}" stroke-linejoin="round"/>'
    if kind == "people":
        return (f'<circle cx="{cx-s*0.18}" cy="{cy-s*0.15}" r="{s*0.15}" fill="none" stroke="{col}" stroke-width="{s*0.09}"/>'
                f'<circle cx="{cx+s*0.2}" cy="{cy-s*0.2}" r="{s*0.13}" fill="none" stroke="{col}" stroke-width="{s*0.09}"/>'
                f'<path d="M{cx-s*0.45} {cy+s*0.4} Q{cx-s*0.18} {cy-s*0.05} {cx+s*0.1} {cy+s*0.4} M{cx+s*0.05} {cy+s*0.1} Q{cx+s*0.25} {cy-s*0.05} {cx+s*0.45} {cy+s*0.35}" fill="none" stroke="{col}" stroke-width="{s*0.09}" stroke-linecap="round"/>')
    if kind == "chart":
        return "".join(f'<rect x="{cx-s*0.4+i*s*0.29}" y="{cy+s*0.4-hh*s}" width="{s*0.2}" height="{hh*s}" rx="{s*0.04}" fill="{col}"/>' for i, hh in enumerate([0.4, 0.7, 0.55]))
    if kind == "menu":
        return "".join(f'<rect x="{cx-s*0.4}" y="{cy-s*0.3+i*s*0.27}" width="{s*0.8}" height="{s*0.1}" rx="{s*0.05}" fill="{col}"/>' for i in range(3))
    return ""


def hud(w, h):
    """Mobile HUD mock: full-screen map, resource strip, minimap card, bottom bar, unit card."""
    p = [map_scene(w, h, seed=7)]
    bar = h * 0.15
    # resource strip
    p.append(f'<rect x="{w*0.02:.0f}" y="{h*0.04:.0f}" width="{w*0.36:.0f}" height="{h*0.09:.0f}" rx="{h*0.045:.0f}" fill="#0b1222" opacity="0.82"/>')
    for i, (c, n) in enumerate([("#c99a5b", "42"), ("#a7a7a7", "18"), ("#8a5a32", "27"), ("#e7c35a", "64")]):
        cx = w * 0.05 + i * w * 0.085
        p.append(f'<rect x="{cx-h*0.022:.1f}" y="{h*0.065:.1f}" width="{h*0.044:.1f}" height="{h*0.044:.1f}" rx="2" fill="{c}"/>'
                 f'<text x="{cx+h*0.035:.1f}" y="{h*0.1:.1f}" font-family="{SANS}" font-weight="600" font-size="{h*0.05:.1f}" fill="{TEXT}">{n}</text>')
    # minimap
    mm = h * 0.26
    p.append(f'<rect x="{w*0.02:.0f}" y="{h-bar-mm-h*0.04:.0f}" width="{mm*1.3:.0f}" height="{mm:.0f}" rx="8" fill="#0b1222" opacity="0.88"/>'
             f'<path d="M{w*0.02+mm*0.25:.0f} {h-bar-h*0.07:.0f} l{mm*0.25:.0f} -{mm*0.8:.0f} h{mm*0.7:.0f} l-{mm*0.25:.0f} {mm*0.8:.0f} Z" fill="#5b9a3a"/>'
             f'<path d="M{w*0.02+mm*0.55:.0f} {h-bar-h*0.2:.0f} q{mm*0.2:.0f} -{mm*0.3:.0f} {mm*0.5:.0f} -{mm*0.2:.0f}" stroke="#5aa6e6" stroke-width="3" fill="none"/>'
             f'<rect x="{w*0.02+mm*0.5:.0f}" y="{h-bar-mm*0.75:.0f}" width="{mm*0.35:.0f}" height="{mm*0.25:.0f}" fill="none" stroke="#fff" stroke-width="1.5"/>')
    # unit card (right)
    p.append(f'<rect x="{w*0.7:.0f}" y="{h*0.05:.0f}" width="{w*0.28:.0f}" height="{h*0.22:.0f}" rx="10" fill="#0b1222" opacity="0.88"/>'
             f'<text x="{w*0.72:.0f}" y="{h*0.12:.0f}" font-family="{SANS}" font-weight="700" font-size="{h*0.05:.1f}" fill="{TEXT}">12 selected</text>'
             f'<rect x="{w*0.72:.0f}" y="{h*0.15:.0f}" width="{w*0.115:.0f}" height="{h*0.08:.0f}" rx="6" fill="{GREEN}"/>'
             f'<text x="{w*0.777:.0f}" y="{h*0.205:.0f}" text-anchor="middle" font-family="{SANS}" font-weight="700" font-size="{h*0.04:.1f}" fill="#fff">Move</text>'
             f'<rect x="{w*0.845:.0f}" y="{h*0.15:.0f}" width="{w*0.115:.0f}" height="{h*0.08:.0f}" rx="6" fill="#33415f"/>'
             f'<text x="{w*0.902:.0f}" y="{h*0.205:.0f}" text-anchor="middle" font-family="{SANS}" font-weight="700" font-size="{h*0.04:.1f}" fill="#fff">Cancel</text>')
    # selection rings
    for sx, sy in [(0.5, 0.55), (0.53, 0.6), (0.47, 0.61), (0.51, 0.66)]:
        p.append(f'<ellipse cx="{w*sx:.0f}" cy="{h*sy:.0f}" rx="{h*0.03:.1f}" ry="{h*0.015:.1f}" fill="none" stroke="{GOLD}" stroke-width="1.6"/>'
                 f'<circle cx="{w*sx:.0f}" cy="{h*sy-h*0.02:.0f}" r="{h*0.016:.1f}" fill="#3b6fe0" stroke="#fff" stroke-width="0.8"/>')
    # tap ripple
    tx, ty = w * 0.36, h * 0.66
    p.append(f'<circle cx="{tx:.0f}" cy="{ty:.0f}" r="{h*0.05:.0f}" fill="#fff" opacity="0.25"/>'
             f'<circle cx="{tx:.0f}" cy="{ty:.0f}" r="{h*0.09:.0f}" fill="none" stroke="#fff" stroke-width="2" opacity="0.5"/>'
             f'<path d="M{w*0.5:.0f} {h*0.6:.0f} Q{w*0.43:.0f} {h*0.58:.0f} {tx+h*0.05:.0f} {ty-h*0.02:.0f}" fill="none" stroke="#fff" stroke-width="2" stroke-dasharray="3 5"/>')
    # bottom bar
    p.append(f'<rect x="0" y="{h-bar:.0f}" width="{w}" height="{bar:.0f}" fill="#0b1222" opacity="0.92"/>')
    kinds = ["house", "wheat", "sword", "star", "people", "chart", "menu"]
    bw = w / len(kinds)
    for i, k in enumerate(kinds):
        bx = i * bw
        fill = "#2b3c66" if i == 0 else "#1a2645"
        p.append(f'<rect x="{bx+bw*0.08:.1f}" y="{h-bar+bar*0.12:.1f}" width="{bw*0.84:.1f}" height="{bar*0.76:.1f}" rx="8" fill="{fill}"/>')
        p.append(glyph(k, bx + bw / 2, h - bar / 2, bar * 0.5, GOLD if i == 0 else TEXT))
    return "".join(p)


def classic(w, h):
    """Classic Touch mock: map with the side panel, magnifier bubble, context chip."""
    pw = w * 0.22
    p = [f'<g transform="translate({pw:.0f} 0)">{map_scene(w-pw, h, seed=11)}</g>']
    p.append(f'<rect width="{pw:.0f}" height="{h}" fill="#2b2117"/>'
             f'<rect x="{pw*0.08:.0f}" y="{h*0.04:.0f}" width="{pw*0.84:.0f}" height="{pw*0.6:.0f}" fill="#4f8a32" stroke="#8a6a3a" stroke-width="2"/>')
    cols, rows = 3, 5
    cw = pw * 0.84 / cols
    top = h * 0.04 + pw * 0.7
    for r in range(rows):
        for c in range(cols):
            hl = (r, c) == (1, 1)
            p.append(f'<rect x="{pw*0.08+c*cw+2:.1f}" y="{top+r*cw*0.95:.1f}" width="{cw-4:.1f}" height="{cw*0.95-4:.1f}" rx="3" '
                     f'fill="#4a3a28" stroke="{GOLD if hl else "#6d5638"}" stroke-width="{3 if hl else 1}"/>')
            p.append(glyph("house", pw * 0.08 + c * cw + cw / 2, top + r * cw * 0.95 + cw * 0.45, cw * 0.45, "#e9dcc0"))
    # finger on the highlighted slot + magnifier bubble
    fx, fy = pw * 0.08 + 1.5 * cw, top + 1.45 * cw * 0.95
    p.append(f'<circle cx="{fx:.0f}" cy="{fy:.0f}" r="{cw*0.42:.0f}" fill="#fff" opacity="0.3"/>')
    bx, by, bw2, bh = pw + w * 0.03, h * 0.18, w * 0.26, h * 0.34
    p.append(f'<path d="M{fx+cw*0.4:.0f} {fy:.0f} L{bx:.0f} {by+bh*0.4:.0f} L{bx:.0f} {by+bh*0.7:.0f} Z" fill="#0b1222" opacity="0.6"/>'
             f'<rect x="{bx:.0f}" y="{by:.0f}" width="{bw2:.0f}" height="{bh:.0f}" rx="12" fill="#0b1222" opacity="0.92" stroke="{GOLD}" stroke-width="2"/>'
             f'<rect x="{bx+bw2*0.25:.0f}" y="{by+bh*0.1:.0f}" width="{bw2*0.5:.0f}" height="{bh*0.5:.0f}" rx="6" fill="#4a3a28" stroke="{GOLD}" stroke-width="3"/>')
    p.append(glyph("house", bx + bw2 / 2, by + bh * 0.35, bh * 0.36, "#e9dcc0"))
    p.append(f'<text x="{bx+bw2/2:.0f}" y="{by+bh*0.82:.0f}" text-anchor="middle" font-family="{SANS}" font-weight="700" font-size="{h*0.055:.1f}" fill="{TEXT}">Woodcutter</text>')
    # context chip + cancel
    p.append(f'<rect x="{w*0.6:.0f}" y="{h*0.05:.0f}" width="{w*0.24:.0f}" height="{h*0.09:.0f}" rx="{h*0.045:.0f}" fill="#0b1222" opacity="0.85"/>'
             f'<text x="{w*0.72:.0f}" y="{h*0.11:.0f}" text-anchor="middle" font-family="{SANS}" font-weight="600" font-size="{h*0.045:.1f}" fill="{GOLD}">Place: Woodcutter</text>'
             f'<circle cx="{w*0.92:.0f}" cy="{h*0.095:.0f}" r="{h*0.055:.0f}" fill="{RED}"/>'
             f'<path d="M{w*0.92-h*0.022:.0f} {h*0.073:.0f} l{h*0.044:.0f} {h*0.044:.0f} m0 -{h*0.044:.0f} l-{h*0.044:.0f} {h*0.044:.0f}" stroke="#fff" stroke-width="3" stroke-linecap="round"/>')
    return "".join(p)


# ---------------------------------------------------------------- files


def hero():
    W, H = 1280, 640
    b = [f'<rect width="{W}" height="{H}" fill="url(#bg)"/><rect width="{W}" height="{H}" fill="url(#hex)"/>']
    b.append(f'<ellipse cx="930" cy="330" rx="420" ry="260" fill="url(#glow)"/>')
    b.append(emblem(84, 104, 112))
    b.append(f'<text x="80" y="330" font-family="{SANS}" font-weight="800" font-size="92" fill="url(#gold)" letter-spacing="-2">S3 Mobile</text>')
    b.append(f'<text x="84" y="392" font-family="{SANS}" font-weight="600" font-size="34" fill="{TEXT}">The Settlers III, in your pocket.</text>')
    b.append(f'<text x="84" y="436" font-family="{SANS}" font-size="22" fill="{MUTED}">Your own copy. The original game. Made for touch.</text>')
    x = 84
    for label, col in [("Android", GREEN), ("iPhone", "#8fa2c8"), ("Free &amp; open source", GOLD)]:
        wpx = 26 + len(label.replace("&amp;", "&")) * 11.5
        b.append(f'<rect x="{x}" y="470" width="{wpx:.0f}" height="38" rx="19" fill="none" stroke="{col}" stroke-width="2"/>'
                 f'<circle cx="{x+18}" cy="489" r="5" fill="{col}"/>'
                 f'<text x="{x+30}" y="496" font-family="{SANS}" font-weight="600" font-size="18" fill="{TEXT}">{label}</text>')
        x += wpx + 14
    b.append(phone(640, 175, 600, 300, hud, "heroclip", rot=-5))
    write("hero.svg", svg(W, H, "".join(b), BG_DEFS))


def modes():
    W, H = 1280, 520
    b = [f'<rect width="{W}" height="{H}" rx="24" fill="url(#bg)"/><rect width="{W}" height="{H}" rx="24" fill="url(#hex)"/>']
    for i, (title, sub, fn) in enumerate([
        ("Classic Touch", "The original panel, sized for fingers", classic),
        ("Mobile HUD", "Full-screen map with a phone-style bar", hud),
    ]):
        x0 = 50 + i * 615
        b.append(phone(x0, 60, 565, 285, fn, f"mclip{i}"))
        b.append(f'<text x="{x0+282}" y="410" text-anchor="middle" font-family="{SANS}" font-weight="800" font-size="34" fill="url(#gold)">{title}</text>'
                 f'<text x="{x0+282}" y="450" text-anchor="middle" font-family="{SANS}" font-size="21" fill="{MUTED}">{sub}</text>')
    b.append(f'<text x="{W/2}" y="496" text-anchor="middle" font-family="{SANS}" font-size="17" fill="{MUTED}" opacity="0.8">Switch any time in the game menu</text>')
    write("ui-modes.svg", svg(W, H, "".join(b), BG_DEFS))


def steps():
    W, H = 1280, 400
    b = [f'<rect width="{W}" height="{H}" rx="24" fill="url(#bg)"/><rect width="{W}" height="{H}" rx="24" fill="url(#hex)"/>']
    cards = [
        ("1", "Install the app", "Android .apk or iPhone .ipa"),
        ("2", "Copy your game", "From your PC over Wi-Fi"),
        ("3", "Play", "Campaigns, maps, saves: all yours"),
    ]
    for i, (n, t, s) in enumerate(cards):
        x0 = 40 + i * 410
        b.append(f'<rect x="{x0}" y="40" width="380" height="320" rx="20" fill="{CARD}" stroke="{LINE}" stroke-width="2"/>')
        b.append(f'<circle cx="{x0+44}" cy="84" r="22" fill="url(#gold)"/><text x="{x0+44}" y="93" text-anchor="middle" font-family="{SANS}" font-weight="800" font-size="24" fill="{NAVY}">{n}</text>')
        cx, cy = x0 + 190, 175
        if i == 0:
            b.append(f'<rect x="{cx-45}" y="{cy-75}" width="90" height="150" rx="14" fill="#0a0d14" stroke="#3a4766" stroke-width="3"/>'
                     f'<rect x="{cx-36}" y="{cy-62}" width="72" height="124" rx="6" fill="#1e3a6e"/>'
                     f'<path d="M{cx} {cy-38} v52 M{cx-22} {cy-6} l22 22 l22 -22" fill="none" stroke="{GOLD}" stroke-width="8" stroke-linecap="round" stroke-linejoin="round"/>'
                     f'<rect x="{cx-24}" y="{cy+28}" width="48" height="8" rx="4" fill="{GOLD}"/>')
        elif i == 1:
            b.append(f'<rect x="{cx-150}" y="{cy-50}" width="130" height="85" rx="8" fill="#0a0d14" stroke="#3a4766" stroke-width="3"/>'
                     f'<rect x="{cx-141}" y="{cy-42}" width="112" height="68" rx="3" fill="#1e3a6e"/>'
                     f'<path d="M{cx-165} {cy+40} h160 l-10 10 h-140 Z" fill="#3a4766"/>'
                     f'<path d="M{cx-112} {cy-22} h24 l6 6 h26 v30 h-56 Z" fill="{GOLD}"/>')
            for r in (14, 28, 42):
                b.append(f'<path d="M{cx+5-r*0.0} {cy-10} m-{r*0.7:.1f} 0 a{r} {r} 0 0 1 {r*1.4:.1f} 0" transform="rotate(90 {cx+5} {cy-10})" fill="none" stroke="{GOLD}" stroke-width="5" stroke-linecap="round" opacity="{1-r/70:.2f}"/>')
            b.append(f'<rect x="{cx+60}" y="{cy-65}" width="74" height="128" rx="12" fill="#0a0d14" stroke="#3a4766" stroke-width="3"/>'
                     f'<rect x="{cx+67}" y="{cy-55}" width="60" height="108" rx="5" fill="#1e3a6e"/>'
                     f'<rect x="{cx+77}" y="{cy-10}" width="40" height="8" rx="4" fill="#33415f"/><rect x="{cx+77}" y="{cy-10}" width="28" height="8" rx="4" fill="{GREEN}"/>')
        else:
            b.append(f'<g transform="translate({cx-110} {cy-58})"><rect width="220" height="116" rx="18" fill="#0a0d14" stroke="#3a4766" stroke-width="3"/>'
                     f'<clipPath id="stepclip"><rect x="8" y="8" width="204" height="100" rx="10"/></clipPath>'
                     f'<g clip-path="url(#stepclip)"><g transform="translate(8 8)">{map_scene(204, 100, seed=5)}</g></g></g>'
                     f'<circle cx="{cx}" cy="{cy}" r="30" fill="{GREEN}" stroke="#fff" stroke-width="3"/>'
                     f'<path d="M{cx-9} {cy-15} L{cx+16} {cy} L{cx-9} {cy+15} Z" fill="#fff"/>')
        b.append(f'<text x="{x0+190}" y="300" text-anchor="middle" font-family="{SANS}" font-weight="800" font-size="30" fill="{TEXT}">{t}</text>'
                 f'<text x="{x0+190}" y="334" text-anchor="middle" font-family="{SANS}" font-size="19" fill="{MUTED}">{s}</text>')
        if i < 2:
            b.append(f'<path d="M{x0+384} 200 l22 0 m-8 -8 l8 8 l-8 8" fill="none" stroke="{GOLD}" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>')
    write("steps.svg", svg(W, H, "".join(b), BG_DEFS))


def finger(cx, cy, r=18, ring=True):
    s = f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="{GOLD}" opacity="0.9"/>'
    if ring:
        s += f'<circle cx="{cx}" cy="{cy}" r="{r+12}" fill="none" stroke="{GOLD}" stroke-width="3" opacity="0.45"/>'
    return s


def arrow(x1, y1, x2, y2, dash=False):
    import math
    a = math.atan2(y2 - y1, x2 - x1)
    hx1, hy1 = x2 - 12 * math.cos(a - 0.5), y2 - 12 * math.sin(a - 0.5)
    hx2, hy2 = x2 - 12 * math.cos(a + 0.5), y2 - 12 * math.sin(a + 0.5)
    d = ' stroke-dasharray="6 7"' if dash else ""
    return (f'<path d="M{x1} {y1} L{x2} {y2}" stroke="{TEXT}" stroke-width="4" stroke-linecap="round"{d}/>'
            f'<path d="M{hx1:.1f} {hy1:.1f} L{x2} {y2} L{hx2:.1f} {hy2:.1f}" fill="none" stroke="{TEXT}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>')


def gestures():
    W, H = 1280, 300
    b = [f'<rect width="{W}" height="{H}" rx="24" fill="url(#bg)"/>']
    items = [
        ("Tap", "select, move, build"),
        ("Drag", "move the map"),
        ("Pinch", "zoom in and out"),
        ("Hold + drag", "select many units"),
        ("Two-finger tap", "cancel"),
    ]
    for i, (t, s) in enumerate(items):
        x0 = 24 + i * 248
        cx, cy = x0 + 118, 115
        b.append(f'<rect x="{x0}" y="24" width="236" height="252" rx="18" fill="{CARD}" stroke="{LINE}" stroke-width="2"/>')
        if i == 0:
            b.append(finger(cx, cy) + f'<circle cx="{cx}" cy="{cy}" r="48" fill="none" stroke="{GOLD}" stroke-width="2" opacity="0.2"/>')
        elif i == 1:
            b.append(finger(cx - 40, cy + 20, ring=False) + arrow(cx - 18, cy + 8, cx + 52, cy - 30, dash=True))
        elif i == 2:
            b.append(finger(cx - 22, cy + 22, ring=False) + finger(cx + 22, cy - 22, ring=False)
                     + arrow(cx - 40, cy + 40, cx - 66, cy + 66) + arrow(cx + 40, cy - 40, cx + 66, cy - 66))
        elif i == 3:
            b.append(f'<rect x="{cx-56}" y="{cy-44}" width="112" height="88" fill="{GOLD}" fill-opacity="0.12" stroke="{GOLD}" stroke-width="3" stroke-dasharray="8 6"/>'
                     + finger(cx - 56, cy - 44) + finger(cx + 56, cy + 44, ring=False))
        else:
            b.append(finger(cx - 26, cy) + finger(cx + 26, cy))
        b.append(f'<text x="{cx}" y="218" text-anchor="middle" font-family="{SANS}" font-weight="800" font-size="25" fill="{TEXT}">{t}</text>'
                 f'<text x="{cx}" y="250" text-anchor="middle" font-family="{SANS}" font-size="18" fill="{MUTED}">{s}</text>')
    write("gestures.svg", svg(W, H, "".join(b), BG_DEFS))


ICONS = {
    "original": f'<path d="M22 70 V38 h8 v-8 h8 v8 h8 v-8 h8 v8 h8 v-8 h8 v8 h8 V70 Z" fill="none" stroke="{GOLD}" stroke-width="5" stroke-linejoin="round"/><path d="M44 70 v-14 a6 6 0 0 1 12 0 v14" fill="none" stroke="{GOLD}" stroke-width="5"/><path d="M50 30 V14 l14 5 l-14 5" fill="{GOLD}" stroke="{GOLD}" stroke-width="3" stroke-linejoin="round"/>',
    "touch": f'<circle cx="50" cy="44" r="10" fill="{GOLD}"/><circle cx="50" cy="44" r="22" fill="none" stroke="{GOLD}" stroke-width="4" opacity="0.6"/><circle cx="50" cy="44" r="34" fill="none" stroke="{GOLD}" stroke-width="3" opacity="0.3"/><path d="M50 56 v24" stroke="{GOLD}" stroke-width="5" stroke-linecap="round"/>',
    "wifi": f'<path d="M20 44 a42 42 0 0 1 60 0 M30 55 a28 28 0 0 1 40 0 M40 66 a14 14 0 0 1 20 0" fill="none" stroke="{GOLD}" stroke-width="6" stroke-linecap="round"/><circle cx="50" cy="76" r="5" fill="{GOLD}"/>',
    "screen": f'<rect x="14" y="28" width="72" height="44" rx="8" fill="none" stroke="{GOLD}" stroke-width="5"/><path d="M26 50 h-8 m8 0 l-5 -5 m5 5 l-5 5 M74 50 h8 m-8 0 l5 -5 m-5 5 l5 5" stroke="{GOLD}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/><path d="M36 50 h28" stroke="{GOLD}" stroke-width="4" stroke-dasharray="3 5" stroke-linecap="round"/>',
    "multi": f'<rect x="10" y="32" width="30" height="48" rx="6" fill="none" stroke="{GOLD}" stroke-width="5"/><rect x="60" y="32" width="30" height="48" rx="6" fill="none" stroke="{GOLD}" stroke-width="5"/><path d="M44 50 h12 m-5 -5 l5 5 l-5 5 M56 62 h-12 m5 -5 l-5 5 l5 5" stroke="{GOLD}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>',
    "save": f'<path d="M22 22 h46 l12 12 v46 h-58 Z" fill="none" stroke="{GOLD}" stroke-width="5" stroke-linejoin="round"/><rect x="34" y="22" width="26" height="16" fill="none" stroke="{GOLD}" stroke-width="4"/><path d="M40 68 V52 l16 -4 v16" fill="none" stroke="{GOLD}" stroke-width="4"/><circle cx="37" cy="68" r="4" fill="{GOLD}"/><circle cx="53" cy="64" r="4" fill="{GOLD}"/>',
}


def icons():
    for name, body in ICONS.items():
        write(f"icon-{name}.svg", svg(100, 100, f'<rect width="100" height="100" rx="24" fill="{NAVY2}" stroke="{LINE}" stroke-width="2"/>{body}'))


if __name__ == "__main__":
    hero()
    modes()
    steps()
    gestures()
    icons()
    print("written to", os.path.normpath(OUT))
