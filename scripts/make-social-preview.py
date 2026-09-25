#!/usr/bin/env python3
"""Render the GitHub social-preview banner and a square 512px icon from the real tray asset.

Both outputs are *derived* from `assets/tray.png` — the same owl the app draws — rather than
redrawn, so the repo's public face and the icon in your tray cannot drift apart.

    python3 scripts/make-social-preview.py

Writes `docs/social-preview.png` (1280x640, for Settings -> General -> Social preview),
`docs/icon-512.png` (square, dark backdrop) and `docs/owl-1024.png` (the plain owl, transparent).

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


def owl_masks() -> tuple[Image.Image, Image.Image]:
    """The owl split into its two inked regions, straight from the shipped pixels.

    The glyph is really only three states per pixel — body, face, nothing — which is what makes a
    crisp upscale possible at all: lift each region out as a mask and the shapes can be re-rendered
    at any size without ever redrawing them.
    """
    src = Image.open(OWL).convert("RGBA")
    px = src.load()
    body = Image.new("L", src.size, 0)
    face = Image.new("L", src.size, 0)
    b, f = body.load(), face.load()
    for y in range(src.height):
        for x in range(src.width):
            r, g, bl, a = px[x, y]
            if a < 128:
                continue
            b[x, y] = 255
            # The face is the light half of a two-tone glyph; the tray_blue variant paints it
            # #00A0FF, so brightness is the wrong test and distance to the dark body is the
            # right one.
            if abs(r - 36) + abs(g - 41) + abs(bl - 47) > 120:
                f[x, y] = 255
    return body, face


def crisp_owl(size: tuple[int, int]) -> Image.Image:
    """The owl at `size`, upscaled without the mush a straight LANCZOS gives.

    A plain resize of a 98px source to 330 is a soft 3.4x, and to 1024 a soft 10x. Each mask is
    instead blown up 4x beyond the target, hard-thresholded there, then averaged back down — a clean
    edge with real anti-aliasing rather than either mush or a staircase.
    """
    w, h = size
    body, face = owl_masks()

    def crisp(mask: Image.Image) -> Image.Image:
        big = mask.resize((w * 4, h * 4), Image.LANCZOS).point(lambda v: 255 if v >= 128 else 0)
        return big.resize((w, h), Image.LANCZOS)

    body_a, face_a = crisp(body), crisp(face)
    out = Image.new("RGBA", (w, h), (36, 41, 47, 0))
    out.paste((36, 41, 47, 255), (0, 0), body_a)
    out.paste((255, 255, 255, 255), (0, 0), face_a)
    return out


# How far past the owl the drop shadow is allowed to bleed, as a fraction of its height.
TILE_BLEED = 0.12


def owl_tile(height: int) -> Image.Image:
    """The owl at `height`, with a soft drop shadow behind it. The layer is larger than the owl.

    **Nothing here draws a plate.** `assets/tray.png` is already a rounded tile: a #24292F plate with
    the white glyph on it. A rounded rectangle used to be drawn at exactly the owl's size and put
    through `GaussianBlur(12)`, meant as a lift under a dark glyph on a dark canvas. What it actually
    did was smear a halo a dozen pixels past the owl's own crisp corners, which is what the banner's
    blurred-corner look was. Drawing that same plate *sharp* is not the fix either — it just reads as
    two nested tiles.

    The blur belongs to a shadow, and a shadow goes behind. The silhouette is the owl's own alpha, so
    it follows the corner radius exactly rather than needing a rounded rectangle guessed to match it.
    """
    owl = Image.open(OWL).convert("RGBA")
    w = round(owl.width * height / owl.height)
    # LANCZOS, not the threshold trick `crisp_owl` uses: at this 3.4x the threshold snaps the contour
    # back onto the 98px source grid and the curve visibly stair-steps. That trick earns its keep at
    # `plain_icon`'s 10x, where a plain resize really is mush.
    owl = owl.resize((w, height), Image.LANCZOS)

    bleed = round(height * TILE_BLEED)
    layer = Image.new("RGBA", (w + bleed * 2, height + bleed * 2), (0, 0, 0, 0))

    drop = round(height * 0.03)
    shadow = Image.new("RGBA", layer.size, (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 140), (bleed, bleed + drop), owl.getchannel("A"))
    layer.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(height * 0.045)))
    layer.alpha_composite(owl, (bleed, bleed))
    return layer


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

    tile = owl_tile(340)
    img.alpha_composite(tile, (150 - round(340 * TILE_BLEED), (h - tile.height) // 2))

    d = ImageDraw.Draw(img)
    x = 560
    d.text((x, 201), "GitHoot", font=font(SANS_BOLD, 88), fill=TITLE, anchor="ls")
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

    tile = owl_tile(340)
    img.alpha_composite(tile, ((s - tile.width) // 2, (s - tile.height) // 2))

    img.convert("RGB").save(OUT_ICON, optimize=True)
    print(f"wrote {OUT_ICON.relative_to(ROOT)} ({s}x{s})")


def plain_icon(size: int = 1024) -> None:
    """The owl on its own, transparent background."""
    out = crisp_owl((size, size))
    path = ROOT / "docs" / f"owl-{size}.png"
    out.save(path, optimize=True)
    print(f"wrote {path.relative_to(ROOT)} ({size}x{size}, transparent)")


if __name__ == "__main__":
    banner()
    square_icon()
    plain_icon()
