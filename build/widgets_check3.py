"""Native widgets E2E v3 — deterministic pixel + log assertions.

Anchors (measured on wg2-before.png @1196x799, window-rect frame):
  - progress fill: near-white (250,250,250) run starting x~232, y~239;
    64% of a ~905px track ends ~x=812.
  - slider: accent-blue track segment x 232..~400 (30% of 0..100 over a
    240px viewport scaled), thumb at its right edge, y~265.
  - rating: 3 filled (yellow) + 2 hollow stars around x 236..285, y~379.
  - buttons row (accent blue blocks) y~236 is BELOW the bar? No — buttons
    are at y~285..300 per screenshot; the -10/+10/重置 blocks are the
    accent rows at y 260..273.

Choreography:
  1. start exe, find window, 3x clamp scroll (stable layout).
  2. screenshot before: measure fill width W1 at the bar's center row.
  3. click "+10" button (accent block #2, center ~ (x=-10 block right + 44)).
  4. screenshot after: fill width W2 must exceed W1 by ~9% of track.
  5. click the 4th star (x ~ 272, y ~ 379): log must show change(rating) 4.
"""
import ctypes
import os
import re
import subprocess
import sys
import time
from ctypes import wintypes

TITLE = "PerryTS × GPUI"
EXE = r"E:\AI-workspace\gpui-perryts\host\target\release\gpui-perryts-host.exe"
LOG = r"E:\AI-workspace\gpui-perryts\build\widgets3.log"
SHOT1 = r"E:\AI-workspace\gpui-perryts\build\wg3-before.png"
SHOT2 = r"E:\AI-workspace\gpui-perryts\build\wg3-after.png"
MOUSEEVENTF_WHEEL = 0x0800

u = ctypes.windll.user32
u.SetProcessDPIAware()


def wheel(hwnd, x, y, notches, delay=0.03):
    u.SetForegroundWindow(hwnd)
    u.SetCursorPos(x, y)
    time.sleep(0.12)
    for _ in range(abs(notches)):
        u.mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -120 if notches > 0 else 120, 0)
        time.sleep(delay)
    time.sleep(0.4)


def click(hwnd, x, y):
    u.SetForegroundWindow(hwnd)
    u.SetCursorPos(x, y)
    time.sleep(0.15)
    u.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.05)
    u.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(0.5)


def shot(hwnd, path):
    from PIL import ImageGrab
    r = wintypes.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(r))
    img = ImageGrab.grab(bbox=(r.left, r.top, r.right, r.bottom))
    img.save(path)
    return img, (r.left, r.top)


def fill_width(img):
    """Width of the near-white progress fill at its widest row."""
    px = img.load()
    w, h = img.size
    best = 0
    for y in range(120, h - 40):
        run = 0
        cur = 0
        for x in range(120, w - 120):
            c = px[x, y][:3]
            if all(v > 235 for v in c):
                cur += 1
                run = max(run, cur)
            else:
                cur = 0
        best = max(best, run)
    return best


def log_text():
    with open(LOG, encoding="utf-8", errors="replace") as f:
        return f.read()


def main():
    logf = open(LOG, "w", encoding="utf-8", errors="replace")
    env = dict(os.environ)
    env["QUICKJS_EMBED"] = "1"
    proc = subprocess.Popen([EXE], stdout=logf, stderr=subprocess.STDOUT, env=env)
    try:
        hwnd = 0
        for _ in range(60):
            hwnd = u.FindWindowW(None, TITLE)
            if hwnd:
                break
            time.sleep(0.1)
        if not hwnd:
            raise SystemExit("window never appeared")
        time.sleep(1.5)

        pt = wintypes.POINT(0, 0)
        u.ClientToScreen(hwnd, ctypes.byref(pt))
        ox, oy = pt.x, pt.y
        wr = wintypes.RECT()
        u.GetWindowRect(hwnd, ctypes.byref(wr))
        wx, wy = wr.left, wr.top

        for _ in range(3):
            wheel(hwnd, ox + 700, oy + 250, 14)

        img1, _ = shot(hwnd, SHOT1)
        w1 = fill_width(img1)

        # The three buttons sit under the progress bar. Find accent blocks
        # in the row band 255..310 and click the 2nd block from the left
        # group (the "+10" button; "重置" is 3rd, "-10" is 1st).
        px = img1.load()
        w, h = img1.size
        blocks = []
        for y in range(250, 320):
            run = 0
            start = None
            for x in range(200, w - 300):
                r, g, b = px[x, y][:3]
                if b > 150 and r < 130 and g < 160:
                    if run == 0:
                        start = x
                    run += 1
                else:
                    if run > 24:
                        blocks.append((start, y, run))
                    run = 0
            if run > 24:
                blocks.append((start, y, run))
        # merge vertically: cluster block starts by x within 30px
        xs = sorted({b[0] for b in blocks})
        merged = []
        for x in xs:
            if merged and x - merged[-1][-1] <= 30:
                merged[-1].append(x)
            else:
                merged.append([x])
        btn_xs = [int(sum(g) / len(g)) for g in merged]
        print("button x centers:", btn_xs)
        if len(btn_xs) < 2:
            print("FAIL: buttons not located")
            return 1
        plus_x = btn_xs[1]
        btn_y = blocks[0][1]

        click(hwnd, wx + plus_x + 12, wy + btn_y)
        time.sleep(0.4)

        img2, _ = shot(hwnd, SHOT2)
        w2 = fill_width(img2)
        print(f"fill width before={w1} after={w2}")
        grew = w2 > w1 + 10

        # star click: find yellow pixels; the 4th star center is to the
        # right of the yellow run's end + ~8px
        px2 = img1.load()
        ys = []
        for y in range(330, 430):
            cnt = 0
            xmin = 10 ** 9
            xmax = 0
            for x in range(150, w - 150):
                r, g, b = px2[x, y][:3]
                if r > 180 and g > 130 and b < 110:
                    cnt += 1
                    xmin = min(xmin, x)
                    xmax = max(xmax, x)
            if cnt > 4:
                ys.append((y, xmin, xmax))
        star_ok = False
        if ys:
            sy = ys[len(ys) // 2][0]
            sx_end = max(y[2] for y in ys)
            # click 4th star: gap between stars ~ 20px; hollow star follows
            click(hwnd, wx + sx_end + 18, wy + sy)
            time.sleep(0.4)
            end = log_text()
            m = re.findall(r"change\(rating\).*?\"value\":\"(\d+)\"", end)
            print("rating events:", m[-3:])
            star_ok = bool(m) and m[-1] == "4"

        print(f"progress grew: {grew}  star-4 change: {star_ok}")
        print("RESULT:", "PASS" if grew and star_ok else "FAIL")
        return 0 if grew and star_ok else 1
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except Exception:
            proc.kill()
        logf.close()


if __name__ == "__main__":
    sys.exit(main())
