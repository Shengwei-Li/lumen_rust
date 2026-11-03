"""
Builds Assets/lumen.ico.

Drawn on a 16x16 grid: an ink tile with chamfered pixel corners, two diagonal viewfinder
brackets, and the signal-orange record dot. Every size is an integer nearest-neighbour
scale of the grid, so pixels stay square from 16 px to 256 px.

    python make_icon.py
"""
import io
import os
import struct

from PIL import Image

INK = (17, 17, 17, 255)
PAPER = (241, 241, 239, 255)
SIGNAL = (255, 59, 0, 255)
CLEAR = (0, 0, 0, 0)
SIZES = (16, 32, 48, 64, 128, 256)


def grid():
    g = [[INK] * 16 for _ in range(16)]
    for x, y in [(0, 0), (15, 0), (0, 15), (15, 15)]:
        g[y][x] = CLEAR
    # Viewfinder brackets: top-left and bottom-right only.
    for cx, cy, dx, dy in [(2, 2, 1, 1), (13, 13, -1, -1)]:
        for i in range(4):
            g[cy][cx + dx * i] = PAPER
            g[cy + dy * i][cx] = PAPER
    # Record dot: 6x6 with the corners knocked out.
    for y in range(5, 11):
        for x in range(5, 11):
            if y in (5, 10) and x in (5, 10):
                continue
            g[y][x] = SIGNAL
    return g


def render(size):
    base = Image.new("RGBA", (16, 16))
    base.putdata([p for row in grid() for p in row])
    return base.resize((size, size), Image.NEAREST)


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    frames = []
    for s in SIZES:
        buf = io.BytesIO()
        render(s).save(buf, "PNG")
        frames.append((s, buf.getvalue()))

    # ICO with PNG-compressed entries (Vista+), one per size.
    out = struct.pack("<HHH", 0, 1, len(frames))
    offset = 6 + 16 * len(frames)
    for s, data in frames:
        out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    out += b"".join(data for _, data in frames)

    with open(os.path.join(here, "lumen.ico"), "wb") as f:
        f.write(out)
    render(256).save(os.path.join(here, "lumen-256.png"))


if __name__ == "__main__":
    main()
