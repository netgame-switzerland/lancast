"""Generates the LANcast icons from assets/icon-source.png (needs Pillow).

A source without alpha gets its outer black background made transparent.

Run from the project root: python tools/make_icons.py
Outputs:
  assets/LANcast.ico        multi-size icon embedded into LANcast.exe (build.rs)
  assets/icon-<N>.png       small PNGs embedded for the window and tray icons
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ASSETS = Path("assets")
PNG_SIZES = [16, 20, 24, 32, 40, 48, 64]
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]

source = Image.open(ASSETS / "icon-source.png")
if "A" not in source.getbands():
    # No transparency: make the black background around the shape transparent
    # by flood-filling from the corners (black inside the shape stays).
    marked = source.convert("RGB")
    w, h = marked.size
    for corner in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]:
        ImageDraw.floodfill(marked, corner, (255, 0, 255), thresh=12)
    alpha = Image.eval(
        ImageChops.difference(marked, Image.new("RGB", marked.size, (255, 0, 255))).convert("L"),
        lambda v: 0 if v == 0 else 255,
    ).filter(ImageFilter.GaussianBlur(0.8))
    source = source.convert("RGB")
    source.putalpha(alpha)
source = source.convert("RGBA")
# Crop the transparent margin so small icons use the full area.
left, top, right, bottom = source.getchannel("A").point(lambda a: 255 if a > 8 else 0).getbbox()
side = max(right - left, bottom - top)
cx, cy = (left + right) // 2, (top + bottom) // 2
square = source.crop((cx - side // 2, cy - side // 2, cx - side // 2 + side, cy - side // 2 + side))

for size in PNG_SIZES:
    square.resize((size, size), Image.Resampling.LANCZOS).save(ASSETS / f"icon-{size}.png", optimize=True)
square.resize((256, 256), Image.Resampling.LANCZOS).save(
    ASSETS / "LANcast.ico", sizes=[(s, s) for s in ICO_SIZES]
)
print("icons written to", ASSETS)
