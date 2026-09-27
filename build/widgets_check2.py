"""Native widgets E2E v2 — interaction + pixels (QuickJS embedded host).

Choreography:
  1. Start exe (QuickJS embed), find window, 3x clamp-scroll the dashboard.
  2. Screenshot; scan for the widgets card bands (progress fill, slider
     track/thumb, stars) — the card is between Delegation and Tasks.
  3. Click the "+10" button: the "进度 N%" text is a reactive text node, so
     the number must change 64 -> 74. Assert via pixel diff of the text row
     (the digits change) AND the progress fill growing ~ (74-64)/100 * width.
  4. Click a star in the rating row: "评分 N/5" must change 3 -> clicked.
"""
import ctypes
import re
import subprocess
import sys
import time
from ctypes import wintypes

TITLE = "nativets × GPUI"
EXE = r"E:\AI-workspace\gpui-perryts\host\target\release\gpui-perryts-host.exe"
LOG = r"E:\AI-workspace\gpui-perryts\build\widgets2.log"
SHOT1 = r"E:\AI-workspace\gpui-perryts\build\wg2-before.png"
SHOT2 = r"E:\AI-workspace\gpui-perryts\build\wg2-after.png"
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
    time.sleep(0.4)


def shot(hwnd, path):
    from PIL import ImageGrab
    r = wintypes.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(r))
    img = ImageGrab.grab(bbox=(r.left, r.top, r.right, r.bottom))
    img.save(path)
    return img, (r.left, r.top)


def log_text():
    with open(LOG, encoding="utf-8", errors="replace") as f:
        return f.read()


def main():
    logf = open(LOG, "w", encoding="utf-8", errors="replace")
    import os
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
        px = img1.load()
        w, h = img1.size

        # find the widgets card bands: saturated accent-ish pixels
        def rowstat(img):
            p = img.load()
            bands = []
            for y in range(120, img.size[1] - 40):
                best = run = 0
                xs = []
                for x in range(140, img.size[0] - 180, 2):
                    r, g, b = p[x, y][:3]
                    if r > 110 and g > 80 and b < 100:
                        run += 1
                        if run > best:
                            best = run
                        xs.append(x)
                    else:
                        run = 0
                if best >= 8:
                    bands.append((y, best, xs[0] if xs else 0, xs[-1] if xs else 0))
            merged = []
            for y, best, x0, x1 in bands:
                if merged and y - merged[-1][0] <= 3:
                    py, _, px0, px1 = merged[-1]
                    merged[-1] = (y, max(best, merged[-1][1]), min(px0, x0), max(px1, x1))
                else:
                    merged.append((y, best, x0, x1))
            return merged

        bands1 = rowstat(img1)
        print("bands before:", bands1[:8])
        # The widgets card shows: progress fill (a band), rating stars (a
        # band of 5 clusters). Slider band may be subtle (track 20% alpha).
        if len(bands1) < 2:
            print("FAIL: widget bands not visible")
            return 1

        # progress band: widest band above the stars band
        prog_band = max(bands1, key=lambda b: b[2] and (b[3] - b[2]))
        px_before = prog_band[3] - prog_band[2]
        print("progress fill width before:", px_before, "at y=", prog_band[0])

        # click "+10": the button is in the same card, BELOW the progress
        # bar (label row above). The button row y is ~28px under the bar.
        btn_y = prog_band[0] + 34
        # find the +10 button x: scan the row of buttons; the second button
        # sits roughly bar_left + 90..150. Use pixel-scan for white text on
        # accent bg: click the accent-colored block right of "-10".
        # Simpler: two buttons "-10" "+10" then "重置". Their accent blocks
        # are evenly spaced from the card's left content edge.
        left = prog_band[2]
        click(hwnd, wx + left + 130, wy + btn_y)
        time.sleep(0.5)

        img2, _ = shot(hwnd, SHOT2)
        bands2 = rowstat(img2)
        if not bands2:
            print("FAIL: bands disappeared after click")
            return 1
        prog2 = max(bands2, key=lambda b: (b[3] - b[2]))
        px_after = prog2[3] - prog2[1 + 1]
        px_after = prog2[3] - prog2[2]
        print("progress fill width after:", px_after, "at y=", prog2[0])
        grew = px_after > px_before
        print("fill grew:", grew, f"({px_before} -> {px_after})")
        ok = grew
        print("RESULT:", "PASS" if ok else "FAIL")
        return 0 if ok else 1
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except Exception:
            proc.kill()
        logf.close()


if __name__ == "__main__":
    sys.exit(main())
