"""Render the Discord corner badges (small_image_playing / small_image_paused).

    python scripts/make_presence_icons.py

Writes assets/presence/playing.png and assets/presence/paused.png (512x512).
Discord crops the small image to a circle and shows it at ~40 px, so the
glyph is big and bold; the ring of marquee bulbs is the Marquee touch:
lit while playing, dimmed while paused.
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

SIZE = 512
SS = 4  # supersampling factor
S = SIZE * SS
OUT = Path(__file__).resolve().parent.parent / "assets" / "presence"


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def gradient_disc(top, bottom):
    """Vertical gradient clipped to a circle."""
    grad = Image.new("RGBA", (S, S))
    px = ImageDraw.Draw(grad)
    for y in range(S):
        px.line([(0, y), (S, y)], fill=lerp(top, bottom, y / (S - 1)) + (255,))
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).ellipse([0, 0, S - 1, S - 1], fill=255)
    out = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    out.paste(grad, (0, 0), mask)
    return out


def bulbs(img, lit):
    """Ring of marquee lights just inside the edge."""
    n = 16
    ring_r = S * 0.405
    bulb_r = S * 0.028
    cx = cy = S / 2
    glow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    top = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    td = ImageDraw.Draw(top)
    for i in range(n):
        a = -math.pi / 2 + i * 2 * math.pi / n
        x, y = cx + ring_r * math.cos(a), cy + ring_r * math.sin(a)
        if lit:
            g = bulb_r * 2.4
            gd.ellipse([x - g, y - g, x + g, y + g], fill=(255, 214, 120, 150))
            td.ellipse([x - bulb_r, y - bulb_r, x + bulb_r, y + bulb_r], fill=(255, 241, 204, 255))
        else:
            td.ellipse([x - bulb_r, y - bulb_r, x + bulb_r, y + bulb_r], fill=(255, 255, 255, 70))
    if lit:
        img.alpha_composite(glow.filter(ImageFilter.GaussianBlur(S * 0.018)))
    img.alpha_composite(top)


def rounded_polygon(draw, pts, radius, fill):
    """Polygon with rounded corners: inset polygon grown by `radius`."""
    cx = sum(p[0] for p in pts) / len(pts)
    cy = sum(p[1] for p in pts) / len(pts)
    # Pull each vertex toward the centroid so the grown shape keeps its size.
    inset = []
    for x, y in pts:
        dx, dy = cx - x, cy - y
        d = math.hypot(dx, dy)
        k = radius * 2 / d
        inset.append((x + dx * k, y + dy * k))
    draw.polygon(inset, fill=fill)
    for i, p in enumerate(inset):
        q = inset[(i + 1) % len(inset)]
        draw.line([p, q], fill=fill, width=round(radius * 2))
        draw.ellipse([p[0] - radius, p[1] - radius, p[0] + radius, p[1] + radius], fill=fill)


def shadow(glyph):
    """Soft drop shadow under the white glyph for depth on bright artwork."""
    alpha = glyph.getchannel("A")
    sh = Image.new("RGBA", (S, S), (10, 12, 40, 0))
    sh.putalpha(alpha.point(lambda v: v * 0.35))
    sh = sh.transform(sh.size, Image.AFFINE, (1, 0, 0, 0, 1, -S * 0.012))
    return sh.filter(ImageFilter.GaussianBlur(S * 0.015))


def play_glyph():
    g = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(g)
    h = S * 0.40  # triangle height (vertical extent)
    w = h * 0.92
    # Optical centring: shift right by ~1/6 of the width.
    cx, cy = S / 2 + w * 0.08, S / 2
    pts = [(cx - w / 2, cy - h / 2), (cx + w / 2, cy), (cx - w / 2, cy + h / 2)]
    rounded_polygon(d, pts, S * 0.035, (255, 255, 255, 255))
    return g


def pause_glyph():
    g = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(g)
    h = S * 0.38
    bw = S * 0.115
    gap = S * 0.08
    r = bw * 0.38
    for x0 in (S / 2 - gap / 2 - bw, S / 2 + gap / 2):
        d.rounded_rectangle([x0, S / 2 - h / 2, x0 + bw, S / 2 + h / 2], radius=r, fill=(255, 255, 255, 255))
    return g


def badge(top, bottom, lit, glyph):
    img = gradient_disc(top, bottom)
    bulbs(img, lit)
    img.alpha_composite(shadow(glyph))
    img.alpha_composite(glyph)
    return img.resize((SIZE, SIZE), Image.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    # Marquee indigo (matches the app icon), lights on.
    badge((112, 124, 250), (72, 82, 226), True, play_glyph()).save(OUT / "playing.png", optimize=True)
    # Dimmed slate, lights off.
    badge((84, 90, 112), (50, 54, 70), False, pause_glyph()).save(OUT / "paused.png", optimize=True)
    print("wrote", OUT / "playing.png", OUT / "paused.png")


if __name__ == "__main__":
    main()
