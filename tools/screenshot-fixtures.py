#!/usr/bin/env python3
"""Generate original geometric posters for the offline documentation catalogue."""

import argparse
import json
from pathlib import Path
import random
import textwrap

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "docs/screenshots/fixtures"


def generate():
    from PIL import Image, ImageDraw, ImageFont

    regular = ImageFont.truetype("DejaVuSans.ttf", 18)
    title_font = ImageFont.truetype("DejaVuSans-Bold.ttf", 32)
    catalog = json.loads((FIXTURES / "catalog.json").read_text())
    for index, anime in enumerate(catalog):
        colors = [tuple(bytes.fromhex(color[1:])) for color in anime["palette"]]
        top, bottom, accent = colors
        image = Image.new("RGB", (480, 706))
        draw = ImageDraw.Draw(image)
        for y in range(706):
            t = y / 705
            draw.line((0, y, 479, y), fill=tuple(round(a + (b-a)*t) for a, b in zip(top, bottom)))
        draw = ImageDraw.Draw(image, "RGBA")
        rng = random.Random(anime["id"])
        for _ in range(38):
            x, y = rng.randrange(25, 455), rng.randrange(24, 430)
            r = rng.choice([1, 1, 2])
            draw.ellipse((x-r, y-r, x+r, y+r), fill=(*accent, 160))
        if anime["motif"] == "orbit":
            draw.ellipse((130, 115, 350, 335), fill=(*accent, 220))
            draw.ellipse((105, 95, 310, 300), fill=(*top, 70))
            draw.ellipse((55, 190, 425, 265), outline=(*accent, 200), width=5)
            draw.ellipse((354, 95, 390, 131), fill=(*accent, 160))
        elif anime["motif"] == "tower":
            draw.ellipse((240, 95, 370, 225), fill=(*accent, 200))
            draw.polygon([(190, 395), (210, 210), (270, 210), (295, 395)], fill=(*top, 255))
            draw.rectangle((190, 190, 290, 215), fill=(*accent, 220))
            draw.polygon([(180, 190), (240, 145), (300, 190)], fill=(*top, 255))
            draw.rectangle((225, 235, 255, 290), fill=(*accent, 190))
        else:
            draw.polygon([(240, 105), (345, 215), (240, 350), (135, 215)], fill=(*accent, 210))
            draw.line([(240, 105), (240, 350), (220, 385), (245, 420)], fill=(*top, 240), width=4)
            draw.line([(135, 215), (345, 215)], fill=(*top, 200), width=3)
        draw.polygon([(0, 415), (100, 360), (235, 430), (355, 375), (480, 425), (480, 706), (0, 706)], fill=(*top, 140))
        draw.text((30, 32), f"{index+1:02d}  /  OFFLINE DEMO", fill=(*accent, 255), font=regular)
        lines = textwrap.wrap(anime["title"], width=18)
        y = 540 - (len(lines)-1)*40
        for line in lines:
            draw.text((30, y), line, fill=(247, 242, 230, 255), font=title_font)
            y += 40
        draw.text((30, 657), "ORIGINAL SAMPLE ARTWORK", fill=(*accent, 255), font=regular)
        image.save(FIXTURES / f"{anime['id']}.png", optimize=True)
    print(f"Generated {len(catalog)} original posters (no network or source images)")


def verify():
    import struct

    catalog = json.loads((FIXTURES / "catalog.json").read_text())
    for anime in catalog:
        path = FIXTURES / f"{anime['id']}.png"
        data = path.read_bytes()
        assert data[:8] == b"\x89PNG\r\n\x1a\n", path
        assert struct.unpack(">II", data[16:24]) == (480, 706), path
    print(f"Verified {len(catalog)} poster PNGs")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["generate", "verify"])
    args = parser.parse_args()
    {"generate": generate, "verify": verify}[args.command]()
