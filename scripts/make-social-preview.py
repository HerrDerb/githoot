#!/usr/bin/env python3
"""Render the GitHub social-preview banner and a square 512px icon from the real tray asset.

Both outputs are *derived* from `assets/tray.png` — the same owl the app draws — rather than
redrawn, so the repo's public face and the icon in your tray cannot drift apart.

    python3 scripts/make-social-preview.py

Writes `docs/social-preview.png` (1280x640, for Settings -> General -> Social preview) and
`docs/icon-512.png` (square, for anywhere an avatar-shaped icon is wanted).

Needs Pillow: `pip install --user Pillow`
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OWL = ROOT / "assets" / "tray.png"
OUT_BANNER = ROOT / "docs" / "social-preview.png"
OUT_ICON = ROOT / "docs" / "icon-512.png"

# Straight out of the composited icons the app draws (see docs/icons/).
CANVAS = (13, 17, 23)  # GitHub dark canvas
CANVAS_2 = (22, 27, 34)
TITLE = (240, 246, 252)
MUTED = (139, 148, 158)
FAINT = (110, 118, 129)
RED = (240, 62, 62)  # review requested
GREEN = (26, 201, 74)  # approved
AMBER = (224, 138, 0)  # changes requested
BLUE = (0, 160, 255)  # unread notifications tint

SANS_BOLD = "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf"
SANS = "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf"


def font(path: str, size: int) -> ImageFont.FreeTypeFont:
    try:
        return ImageFont.truetype(path, size)
    except OSError:
        return ImageFont.load_default(size)


def vertical_gradient(size: tuple[int, int], top: tuple, bottom: tuple) -> Image.Image:
    w, h = size
    grad = Image.new("RGB", (1, h))
    for y in range(h):
        t = y / max(h - 1, 1)
        grad.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)))
    return grad.resize((w, h), Image.BILINEAR)


def glow(size: tuple[int, int], centre: tuple[int, int], radius: int, colour: tuple, alpha: int) -> Image.Image:
    """A soft radial wash, drawn as a blurred disc so no gradient maths is needed."""
    layer = Image.new("RGBA", size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    cx, cy = centre
    d.ellipse((cx - radius, cy - radius, cx + radius, cy + radius), fill=(*colour, alpha))
    return layer.filter(ImageFilter.GaussianBlur(radius * 0.55))


def pill(draw: ImageDraw.ImageDraw, x: int, y: int, label: str, colour: tuple, f) -> int:
    """A chip: a short bar in the mark's own colour, then the word. Returns its right edge."""
    pad_x, bar_w, bar_h, gap = 18, 8, 26, 12
    tw = draw.textlength(label, font=f)
    h = 48
    w = round(pad_x * 2 + bar_w + gap + tw)
    draw.rounded_rectangle((x, y, x + w, y + h), radius=h // 2, fill=(28, 33, 40), outline=(48, 54, 61))
    draw.rounded_rectangle(
        (x + pad_x, y + (h - bar_h) // 2, x + pad_x + bar_w, y + (h + bar_h) // 2),
        radius=bar_w // 2,
        fill=colour,
    )
    draw.text((x + pad_x + bar_w + gap, y + h / 2), label, font=f, fill=MUTED, anchor="lm")
    return x + w


def banner() -> None:
    w, h = 1280, 640
    img = vertical_gradient((w, h), CANVAS, CANVAS_2).convert("RGBA")

    # Depth behind the owl, in the notification tint so the palette stays the app's own.
    img.alpha_composite(glow((w, h), (300, 300), 320, BLUE, 34))
    img.alpha_composite(glow((w, h), (980, 520), 300, RED, 12))

    owl = Image.open(OWL).convert("RGBA")
    oh = 330
    ow = round(owl.width * oh / owl.height)
    owl = owl.resize((ow, oh), Image.LANCZOS)

    # The owl body is #24292F: near-invisible on a dark canvas without a lift under it.
    plate = Image.new("RGBA", (ow, oh), (0, 0, 0, 0))
    ImageDraw.Draw(plate).rounded_rectangle((0, 0, ow - 1, oh - 1), radius=round(oh * 0.22), fill=(255, 255, 255, 26))
    ox, oy = 150, (h - oh) // 2
    img.alpha_composite(plate.filter(ImageFilter.GaussianBlur(12)), (ox, oy))
    img.alpha_composite(owl, (ox, oy))

    d = ImageDraw.Draw(img)
    x = 560
    d.text((x, 201), "GitHoot Tray", font=font(SANS_BOLD, 88), fill=TITLE, anchor="ls")
    d.text((x, 265), "The owl that watches your pull requests", font=font(SANS, 34), fill=MUTED, anchor="ls")
    d.text((x, 313), "and hoots the moment one needs you.", font=font(SANS, 34), fill=MUTED, anchor="ls")

    f = font(SANS_BOLD, 26)
    cx = x
    for label, colour in (("Review", RED), ("Approved", GREEN), ("Changes", AMBER)):
        cx = pill(d, cx, 373, label, colour, f) + 16

    d.text((x, 495), "Linux  ·  Windows  ·  macOS  ·  Rust  ·  public domain", font=font(SANS, 24), fill=FAINT, anchor="ls")

    img.convert("RGB").save(OUT_BANNER, optimize=True)
    print(f"wrote {OUT_BANNER.relative_to(ROOT)} ({img.width}x{img.height})")


def square_icon() -> None:
    s = 512
    img = vertical_gradient((s, s), CANVAS, CANVAS_2).convert("RGBA")
    img.alpha_composite(glow((s, s), (s // 2, s // 2), 240, BLUE, 30))

    owl = Image.open(OWL).convert("RGBA")
    oh = 340
    ow = round(owl.width * oh / owl.height)
    owl = owl.resize((ow, oh), Image.LANCZOS)
    ox, oy = (s - ow) // 2, (s - oh) // 2

    plate = Image.new("RGBA", (ow, oh), (0, 0, 0, 0))
    ImageDraw.Draw(plate).rounded_rectangle((0, 0, ow - 1, oh - 1), radius=round(oh * 0.22), fill=(255, 255, 255, 26))
    img.alpha_composite(plate.filter(ImageFilter.GaussianBlur(12)), (ox, oy))
    img.alpha_composite(owl, (ox, oy))

    img.convert("RGB").save(OUT_ICON, optimize=True)
    print(f"wrote {OUT_ICON.relative_to(ROOT)} ({s}x{s})")


if __name__ == "__main__":
    banner()
    square_icon()
