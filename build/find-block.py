"""Locate a solid-colour UI block inside a screen region (coordinate calibration).

Pure Pillow — no numpy in this environment.
Usage: python find-block.py <png> <x0> <y0> <x1> <y1> <rrggbb>
"""

import sys

from PIL import Image

path = sys.argv[1]
rx0, ry0, rx1, ry1 = (int(a) for a in sys.argv[2:6])
hexcolor = sys.argv[6]
tr = (int(hexcolor[0:2], 16), int(hexcolor[2:4], 16), int(hexcolor[4:6], 16))

region = Image.open(path).convert("RGB").crop((rx0, ry0, rx1, ry1))
px = region.load()
xs = []
ys = []
for y in range(region.height):
    for x in range(region.width):
        p = px[x, y]
        if abs(p[0] - tr[0]) + abs(p[1] - tr[1]) + abs(p[2] - tr[2]) < 40:
            xs.append(x)
            ys.append(y)

if not xs:
    print("none")
    sys.exit(0)

print(
    "px=%d center=(%d,%d) bbox=(%d,%d)-(%d,%d)"
    % (
        len(xs),
        rx0 + sum(xs) // len(xs),
        ry0 + sum(ys) // len(ys),
        rx0 + min(xs),
        ry0 + min(ys),
        rx0 + max(xs),
        ry0 + max(ys),
    )
)
