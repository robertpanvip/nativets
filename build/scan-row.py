"""Print colour segments along selected rows so window/client bounds can be read off."""
import sys

from PIL import Image

im = Image.open(sys.argv[1]).convert("RGB")
print("size", im.size)
px = im.load()
for y in [int(a) for a in sys.argv[2:]]:
    row = [px[x, y] for x in range(im.width)]
    segs = []
    cur = row[0]
    start = 0
    for x in range(1, im.width):
        c = row[x]
        if abs(c[0] - cur[0]) + abs(c[1] - cur[1]) + abs(c[2] - cur[2]) > 24:
            if x - start > 4:
                segs.append((start, x - 1, "#%02x%02x%02x" % cur))
            cur = c
            start = x
    segs.append((start, im.width - 1, "#%02x%02x%02x" % cur))
    print("y=%d" % y, segs[:16])
