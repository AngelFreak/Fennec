"""Mockup vs Fennec: for each name, writes <out>/<name>.png with the mockup
on the left, Fennec on the right, and below them a diff where differing
pixels are red over a faded mockup. Prints the share of pixels that differ.

usage: python3 compare.py <mock-dir> <ours-dir> <out-dir> [names...]
"""
import os
import sys

from PIL import Image, ImageChops, ImageDraw

mock_dir, ours_dir, out_dir = sys.argv[1:4]
names = sys.argv[4:] or sorted(
    n[:-4] for n in os.listdir(ours_dir) if n.endswith(".png") and os.path.exists(os.path.join(mock_dir, n))
)
os.makedirs(out_dir, exist_ok=True)
for name in names:
    a = Image.open(os.path.join(mock_dir, name + ".png")).convert("RGB")
    b = Image.open(os.path.join(ours_dir, name + ".png")).convert("RGB")
    b = b.crop((0, 0, a.width, a.height)) if b.size != a.size else b
    if b.size != a.size:
        canvas = Image.new("RGB", a.size, "white")
        canvas.paste(b, (0, 0))
        b = canvas
    diff = ImageChops.difference(a, b).convert("L").point(lambda v: 255 if v > 40 else 0)
    share = sum(diff.histogram()[255:]) / (a.width * a.height)
    faded = Image.blend(a, Image.new("RGB", a.size, "white"), 0.7)
    red = Image.new("RGB", a.size, (220, 30, 30))
    overlay = Image.composite(red, faded, diff)
    sheet = Image.new("RGB", (a.width * 2 + 8, a.height * 2 + 8), (90, 90, 90))
    sheet.paste(a, (0, 0))
    sheet.paste(b, (a.width + 8, 0))
    sheet.paste(overlay, (0, a.height + 8))
    d = ImageDraw.Draw(sheet)
    d.text((a.width + 16, a.height + 16), f"{name}: {share:.1%} of pixels differ", fill="white")
    sheet.save(os.path.join(out_dir, name + ".png"))
    print(f"{name:28s} {share:6.1%}")
