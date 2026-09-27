"""Off-screen grab of a possibly occluded GPUI window via PrintWindow.

    python build/winshot.py <hwnd> <out.png> [titleSubstring]

GPUI renders with the GPU composition path, so plain BitBlt often gets a
black frame; PW_RENDERFULLCONTENT (2) forces DWM to hand over the real
surface. Used because ImageGrab.grab only sees what is on screen — an
occluded window comes back all black.
"""

import ctypes
import sys
from ctypes import wintypes

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32
user32.SetProcessDPIAware()


def grab(hwnd: int, out: str) -> None:
    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    w, h = rect.right - rect.left, rect.bottom - rect.top

    # BITMAPINFO for a top-down 32bpp DIB
    class BITMAPINFOHEADER(ctypes.Structure):
        _fields_ = [
            ("biSize", wintypes.DWORD),
            ("biWidth", ctypes.c_long),
            ("biHeight", ctypes.c_long),
            ("biPlanes", wintypes.WORD),
            ("biBitCount", wintypes.WORD),
            ("biCompression", wintypes.DWORD),
            ("biSizeImage", wintypes.DWORD),
            ("biXPelsPerMeter", ctypes.c_long),
            ("biYPelsPerMeter", ctypes.c_long),
            ("biClrUsed", wintypes.DWORD),
            ("biClrImportant", wintypes.DWORD),
        ]

    class BITMAPINFO(ctypes.Structure):
        _fields_ = [("bmiHeader", BITMAPINFOHEADER)]

    bmi = BITMAPINFO()
    bmi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bmi.bmiHeader.biWidth = w
    bmi.bmiHeader.biHeight = -h  # top-down
    bmi.bmiHeader.biPlanes = 1
    bmi.bmiHeader.biBitCount = 32
    bmi.bmiHeader.biCompression = 0  # BI_RGB

    screen_dc = user32.GetWindowDC(hwnd)
    mem_dc = gdi32.CreateCompatibleDC(screen_dc)
    bmp = gdi32.CreateDIBSection(mem_dc, ctypes.byref(bmi), 0, ctypes.byref(ctypes.c_void_p()), None, 0)
    gdi32.SelectObject(mem_dc, bmp)
    ok = user32.PrintWindow(hwnd, mem_dc, 2)  # PW_RENDERFULLCONTENT

    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(mem_dc, bmp, 0, h, buf, ctypes.byref(bmi), 0)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mem_dc)
    user32.ReleaseDC(hwnd, screen_dc)

    from PIL import Image

    img = Image.frombuffer("RGBA", (w, h), buf.raw, "raw", "BGRA", 0, 1)
    img.save(out)
    # diversity check: an unrendered/occluded window collapses to 1 color
    colors = set()
    px = img.load()
    for y in range(0, h, 40):
        for x in range(0, w, 40):
            colors.add(px[x, y][:3])
    print("PrintWindow ok=%s %dx%d -> %s  distinct=%d" % (ok, w, h, out, len(colors)))


if __name__ == "__main__":
    hwnd = int(sys.argv[1], 0)
    out = sys.argv[2]
    grab(hwnd, out)
