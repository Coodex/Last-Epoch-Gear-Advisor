"""Render the app icon (1024x1024 PNG) without any imaging library.

Design: dark rounded tile with a soft radial glow, a gold heater shield
(the Paladin), an emerald upward chevron inside it (an upgrade), and a small
violet gem at the top of the shield (Last Epoch's arcane purple).

Run:  py -3.12 tools/make_icon.py   -> app/src-tauri/icons/source.png
Then: cd app && npx tauri icon src-tauri/icons/source.png
"""
from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

SIZE = 1024
SS = 2  # supersampling per axis
OUT = Path(__file__).resolve().parents[1] / "app" / "src-tauri" / "icons" / "source.png"


def lerp(a, b, t):
    return a + (b - a) * t


def mix(c1, c2, t):
    return tuple(lerp(c1[i], c2[i], t) for i in range(3))


def rounded_rect_sdf(x, y, half, radius):
    qx, qy = abs(x) - half + radius, abs(y) - half + radius
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    inside = min(max(qx, qy), 0.0)
    return outside + inside - radius


def shield_sdf(x, y):
    """Heater shield centred at origin: flat top, straight upper sides, pointed bottom.
    Approximated as rect (top) unioned with an ellipse (bottom) then a taper."""
    top, width, mid, bottom = -300.0, 260.0, -40.0, 350.0
    if y < mid:
        # upper part: only the side walls and the flat top are boundaries
        return max(abs(x) - width, top - y)
    # lower part: half-ellipse tapering to the point at `bottom`
    ex, ey = x / width, (y - mid) / (bottom - mid)
    return (math.hypot(ex, ey) - 1.0) * min(width, bottom - mid)


def segment_sdf(px, py, ax, ay, bx, by):
    abx, aby = bx - ax, by - ay
    apx, apy = px - ax, py - ay
    t = max(0.0, min(1.0, (apx * abx + apy * aby) / (abx * abx + aby * aby)))
    return math.hypot(apx - t * abx, apy - t * aby)


def chevron_sdf(x, y):
    """Upward chevron: two thick strokes meeting at the apex."""
    apex = (0.0, -60.0)
    left = (-140.0, 110.0)
    right = (140.0, 110.0)
    d = min(segment_sdf(x, y, *apex, *left), segment_sdf(x, y, *apex, *right))
    return d - 42.0


def gem_sdf(x, y):
    """Diamond above the shield."""
    gx, gy = x, y + 360.0
    return (abs(gx) / 1.0 + abs(gy) / 1.4) - 46.0


def coverage(d, aa=1.5):
    """Anti-aliased inside coverage for a signed distance."""
    return max(0.0, min(1.0, 0.5 - d / aa))


def shade(px, py):
    """Colour at a sample point (pixel space, origin centre, y down)."""
    x, y = px - SIZE / 2, py - SIZE / 2
    tile = coverage(rounded_rect_sdf(x, y, SIZE / 2 - 8, 200.0))
    if tile <= 0.0:
        return (0.0, 0.0, 0.0, 0.0)

    # background: dark charcoal with a warm radial glow behind the shield
    r = math.hypot(x, y - 40) / (SIZE * 0.55)
    base = mix((28, 24, 18), (12, 10, 8), min(1.0, r))
    glow = max(0.0, 1.0 - math.hypot(x, y) / 420.0) ** 2
    col = mix(base, (74, 58, 26), glow * 0.55)

    # shield: gold rim + darker plate with vertical gradient
    ds = shield_sdf(x, y)
    rim = coverage(ds) * (1.0 - coverage(ds + 26.0))
    plate = coverage(ds + 26.0)
    plate_col = mix((44, 34, 22), (24, 18, 12), (y + 300.0) / 640.0)
    col = mix(col, plate_col, plate)
    gold = mix((246, 214, 122), (176, 132, 52), (y + 300.0) / 640.0)
    col = mix(col, gold, rim)
    # rim highlight (top-left light)
    hl = coverage(ds) * max(0.0, -(x + y) / 700.0) * (1.0 - coverage(ds + 12.0))
    col = mix(col, (255, 240, 190), hl * 0.8)

    # chevron: emerald with a lighter top edge
    dc = chevron_sdf(x, y)
    chev = coverage(dc) * plate
    green = mix((92, 232, 168), (24, 158, 104), (y + 130.0) / 260.0)
    col = mix(col, green, chev)
    edge = coverage(dc) * (1.0 - coverage(dc + 10.0)) * plate
    col = mix(col, (190, 255, 220), edge * 0.35)

    # gem
    dg = gem_sdf(x, y)
    gem = coverage(dg)
    gem_col = mix((196, 160, 255), (96, 48, 200), (y + 400.0) / 92.0)
    col = mix(col, gem_col, gem)
    gem_rim = coverage(dg) * (1.0 - coverage(dg + 8.0))
    col = mix(col, (236, 220, 255), gem_rim * 0.6)

    return (col[0], col[1], col[2], 255.0 * tile)


def render():
    rows = []
    inv = 1.0 / (SS * SS)
    for py in range(SIZE):
        row = bytearray([0])
        for px in range(SIZE):
            r = g = b = a = 0.0
            for sy in range(SS):
                for sx in range(SS):
                    c = shade(px + (sx + 0.5) / SS, py + (sy + 0.5) / SS)
                    r += c[0] * c[3]
                    g += c[1] * c[3]
                    b += c[2] * c[3]
                    a += c[3]
            if a > 0:
                r, g, b = r / a, g / a, b / a
            a *= inv
            row += bytes((int(min(255, r)), int(min(255, g)), int(min(255, b)), int(min(255, a))))
        rows.append(bytes(row))
        if py % 128 == 0:
            print(f"row {py}/{SIZE}", flush=True)
    return b"".join(rows)


def write_png(raw: bytes, path: Path) -> None:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


if __name__ == "__main__":
    write_png(render(), OUT)
    print("wrote", OUT)
