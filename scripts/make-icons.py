"""Draws the LRAM app icon from one definition: the flat mark (a light tile with a blue bar down
its left side and a blue foot inside it) on a dark rounded tile.

    python scripts/make-icons.py

Writes src-tauri/icons/{32x32,128x128,128x128@2x,icon}.png and icon.ico. Needs Pillow
(`pip install pillow`). app-icon.svg in the same folder is the same drawing, for anything that
wants a vector.
"""
from pathlib import Path

from PIL import Image, ImageDraw

ICONS = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
SS = 4  # supersampling: Pillow has no anti-aliasing of its own

GROUND = (17, 23, 32, 255)      # #111720
EDGE = (42, 52, 70, 255)        # #2a3446
BLUE = (62, 121, 201, 255)      # #3E79C9
LIGHT = (152, 192, 251, 255)    # #98C0FB

# The mark occupies this share of the tile; the shapes are fractions of the mark.
MARK = (0.115, 0.115, 0.885, 0.885)
BAR = (0.0, 0.0, 0.36, 0.94)    # blue, down the left side
FOOT = (0.36, 0.50, 0.92, 0.94)  # blue, inside the light tile: its top-right square stays light


def place(rect: tuple[float, float, float, float], size: int):
    mx0, my0, mx1, my1 = MARK
    mw, mh = (mx1 - mx0) * size, (my1 - my0) * size
    x0, y0, x1, y1 = rect
    return [round(mx0 * size + x0 * mw), round(my0 * size + y0 * mh), round(mx0 * size + x1 * mw), round(my0 * size + y1 * mh)]


def draw(size: int) -> Image.Image:
    big = size * SS
    im = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    d.rounded_rectangle(
        [0, 0, big - 1, big - 1], radius=round(big * 11 / 50), fill=GROUND, outline=EDGE, width=max(1, big // 512)
    )
    d.rectangle(place((0, 0, 1, 1), big), fill=LIGHT)
    d.rectangle(place(BAR, big), fill=BLUE)
    d.rectangle(place(FOOT, big), fill=BLUE)
    return im.resize((size, size), Image.LANCZOS)


def main() -> None:
    for name, size in [("icon", 512), ("128x128@2x", 256), ("128x128", 128), ("32x32", 32)]:
        draw(size).save(ICONS / f"{name}.png")
    draw(256).save(
        ICONS / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
    )
    print("icons written to", ICONS)


if __name__ == "__main__":
    main()
