"""
Builds docs/banner-light.png and docs/banner-dark.png for the README of the Rust version.

Same vocabulary as the app: paper, ink, one signal colour, numbered sections,
hairline rules. Rendered at 2x for sharp text on high-DPI screens.

    python docs/make_banner.py
"""
import os
import sys

from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets"))
from make_icon import grid  # noqa: E402

S = 2                       # render scale
W, H = 1280 * S, 360 * S
FONTS = r"C:\Windows\Fonts"

THEMES = {
    "light": dict(bg=(241, 241, 239), ink=(17, 17, 17), muted=(122, 122, 118), rule=(214, 214, 210)),
    "dark": dict(bg=(26, 26, 25), ink=(241, 241, 239), muted=(140, 140, 136), rule=(58, 58, 56)),
}


def font(name, size, variation=None):
    f = ImageFont.truetype(os.path.join(FONTS, name), size * S)
    if variation:
        try:
            f.set_variation_by_axes(variation)
        except Exception:
            pass
    return f


def draw(theme):
    t = THEMES[theme]
    img = Image.new("RGB", (W, H), t["bg"])
    d = ImageDraw.Draw(img)

    pad = 64 * S
    # Hairline frame rules, like the app's section dividers.
    d.line([(pad, 56 * S), (W - pad, 56 * S)], fill=t["rule"], width=S)
    d.line([(pad, H - 56 * S), (W - pad, H - 56 * S)], fill=t["rule"], width=S)

    mono = font("consola.ttf", 13)
    d.text((pad, 30 * S), "LUMEN", font=mono, fill=t["muted"], anchor="lm")
    d.text((W - pad, 30 * S), "WINDOWS  ·  RUST  ·  v1.0", font=mono, fill=t["muted"], anchor="rm")

    # Icon: the 16x16 grid at 9 px per cell.
    cell = 9 * S
    icon = Image.new("RGBA", (16, 16))
    icon.putdata([p for row in grid() for p in row])
    icon = icon.resize((16 * cell, 16 * cell), Image.NEAREST)
    iy = (H - icon.height) // 2
    img.paste(icon, (pad, iy), icon)

    x = pad + icon.width + 40 * S
    title = font("SegUIVar.ttf", 76)
    try:
        title.set_variation_by_name("Semibold")
    except Exception:
        title = font("seguisb.ttf", 76) if os.path.exists(os.path.join(FONTS, "seguisb.ttf")) else font("segoeuib.ttf", 76)
    d.text((x - 4 * S, H // 2 - 14 * S), "Lumen", font=title, fill=t["ink"], anchor="ls")
    # The one difference from the C# banner: a mono tag on the title's baseline.
    end = x - 4 * S + int(d.textlength("Lumen", font=title))
    d.text((end + 18 * S, H // 2 - 14 * S), "RUST", font=font("consola.ttf", 18), fill=t["muted"], anchor="ls")

    body = font("SegUIVar.ttf", 22)
    try:
        body.set_variation_by_name("Regular")
    except Exception:
        pass
    d.text((x, H // 2 + 30 * S), "A focused screen recorder for recorded lessons.", font=body, fill=t["muted"], anchor="ls")

    # The four sections of the app, as a quiet index.
    sections = ["Source", "Camera", "Audio", "Output"]
    sx, sy = x, H // 2 + 78 * S
    num = font("consola.ttf", 14)
    lab = font("SegUIVar.ttf", 16)
    try:
        lab.set_variation_by_name("Regular")
    except Exception:
        pass
    for i, s in enumerate(sections):
        d.text((sx, sy), f"0{i + 1}", font=num, fill=t["muted"], anchor="ls")
        sx += 26 * S
        d.text((sx, sy), s, font=lab, fill=t["ink"], anchor="ls")
        sx += int(d.textlength(s, font=lab)) + 36 * S

    # Signal: the only colour, a small square marking "recording".
    r = 5 * S
    cx, cy = W - pad - r, H - 30 * S
    d.rectangle([cx - r, cy - r, cx + r, cy + r], fill=(255, 59, 0))
    d.text((cx - r - 12 * S, cy), "REC", font=mono, fill=t["muted"], anchor="rm")

    return img


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    for theme in THEMES:
        draw(theme).save(os.path.join(here, f"banner-{theme}.png"), optimize=True)


if __name__ == "__main__":
    main()
