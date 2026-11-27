#!/usr/bin/env python3
"""Render the terminal demo's interface as an SVG screenshot.

Runs docs/demo.sh in a pseudo-terminal, waits for `hs tui` to draw the
matrix with the demo's focus session, records the screen with the pyte
terminal emulator, quits, and writes an SVG in the trivial.si palette.

Usage:
    python3 scripts/render-demo-screenshot.py HS_EXECUTABLE OUTPUT.svg

Requires `pyte` (pip install pyte). The demo uses its own temporary
database, so no personal data can appear in the image.
"""

import os
import pty
import select
import struct
import sys
import time
from html import escape
from pathlib import Path

import fcntl
import pyte
import termios

COLS, ROWS = 112, 34
FONT_SIZE = 15
CELL_W = FONT_SIZE * 0.602  # DejaVu Sans Mono advance width
CELL_H = 19
PAD = 22
BAR = 34

BACKGROUND = "#152a40"
FOREGROUND = "#f7f8f5"
PALETTE = {
    "black": "#152a40",
    "red": "#ff622e",
    "green": "#8fd18b",
    "yellow": "#ffb35c",
    "blue": "#7aa7d9",
    "magenta": "#d29be0",
    "cyan": "#6cc7e0",
    "white": "#c7ced4",
    "brightblack": "#6b7580",
    "brightred": "#ff8a5f",
    "brightgreen": "#aee3a8",
    "brightyellow": "#ffcf8a",
    "brightblue": "#a3c4ea",
    "brightmagenta": "#e3bdec",
    "brightcyan": "#a2dcec",
    "brightwhite": "#ffffff",
    "gray": "#9aa5b1",
    "grey": "#9aa5b1",
}


# Box-drawing characters as the arms (left, right, up, down) they connect.
BOX_ARMS = {
    "\u2500": "lr", "\u2502": "ud", "\u250c": "rd", "\u2510": "ld",
    "\u2514": "ru", "\u2518": "lu", "\u256d": "rd", "\u256e": "ld",
    "\u2570": "ru", "\u256f": "lu", "\u251c": "rud", "\u2524": "lud",
    "\u252c": "lrd", "\u2534": "lru", "\u253c": "lrud",
}


# Block elements as the (top, bottom) fraction of the cell they fill.
BLOCKS = {"\u2588": (0, 1), "\u2580": (0, 0.5), "\u2584": (0.5, 1)}


def box_path(ch, x, y):
    """Line segments from the cell centre, so borders join without gaps."""
    cx, cy = x + CELL_W / 2, y + CELL_H / 2
    ends = {"l": (x, cy), "r": (x + CELL_W, cy), "u": (cx, y), "d": (cx, y + CELL_H)}
    return "".join(
        f"M{cx:.2f} {cy:.2f}L{ends[arm][0]:.2f} {ends[arm][1]:.2f}" for arm in BOX_ARMS[ch]
    )


def color(value, default):
    if value == "default":
        return default
    if value in PALETTE:
        return PALETTE[value]
    if len(value) == 6 and all(c in "0123456789abcdefABCDEF" for c in value):
        return "#" + value.lower()
    return default


def capture(hs, demo):
    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.ByteStream(screen)
    env = dict(os.environ, HS=hs, TERM="xterm-256color")
    env.pop("NO_COLOR", None)
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe("sh", ["sh", str(demo)], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

    def pump(seconds):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if ready:
                try:
                    stream.feed(os.read(fd, 65536))
                except OSError:
                    return

    deadline = time.time() + 20
    while time.time() < deadline:
        pump(0.3)
        text = "\n".join(screen.display)
        if "Lumen Bakery" in text and "inbox" in text.lower() and "focus" in text.lower():
            break
    else:
        raise SystemExit("the terminal interface did not appear:\n" + "\n".join(screen.display))
    pump(1.0)
    snapshot = [[screen.buffer[y][x] for x in range(COLS)] for y in range(ROWS)]
    os.write(fd, b"q")
    pump(2.0)
    os.waitpid(pid, 0)
    return snapshot


def render(rows):
    width = PAD * 2 + COLS * CELL_W
    height = BAR + PAD * 2 + ROWS * CELL_H
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0f}" height="{height:.0f}" '
        f'viewBox="0 0 {width:.0f} {height:.0f}" role="img" aria-labelledby="title desc">',
        "  <title id=\"title\">hs terminal interface demo</title>",
        "  <desc id=\"desc\">The hs terminal interface showing a running 25-minute focus session "
        "for a fictional task above an Eisenhower matrix of fictional tasks and an inbox.</desc>",
        f'  <rect width="{width:.0f}" height="{height:.0f}" rx="12" fill="{BACKGROUND}"/>',
        f'  <path d="M0 {BAR} H{width:.0f}" stroke="#2a4058" stroke-width="1"/>',
        '  <circle cx="22" cy="17" r="6" fill="#ff622e"/>',
        '  <circle cx="42" cy="17" r="6" fill="#6b7580"/>',
        '  <circle cx="62" cy="17" r="6" fill="#6b7580"/>',
        f'  <text x="{width / 2:.0f}" y="22" font-family="Roboto, Arial, sans-serif" font-size="14" '
        f'fill="#c7ced4" text-anchor="middle">hs tui</text>',
        f'  <g font-family="DejaVu Sans Mono, Liberation Mono, monospace" font-size="{FONT_SIZE}" '
        'xml:space="preserve">',
    ]
    top = BAR + PAD
    for y, row in enumerate(rows):
        baseline = top + y * CELL_H + CELL_H * 0.76
        x = 0
        while x < COLS:
            cell = row[x]
            fg = color(cell.fg, FOREGROUND)
            bg = color(cell.bg, BACKGROUND)
            if cell.reverse:
                fg, bg = bg, fg
            key = (fg, bg, cell.bold)
            start = x
            text = ""
            while x < COLS:
                c = row[x]
                cfg = color(c.fg, FOREGROUND)
                cbg = color(c.bg, BACKGROUND)
                if c.reverse:
                    cfg, cbg = cbg, cfg
                if (cfg, cbg, c.bold) != key:
                    break
                text += c.data or ""
                x += 1
            px = PAD + start * CELL_W
            span = (x - start) * CELL_W
            if bg != BACKGROUND:
                out.append(
                    f'    <rect x="{px:.2f}" y="{top + y * CELL_H}" width="{span:.2f}" '
                    f'height="{CELL_H}" fill="{bg}"/>'
                )
            # Blocks and borders are drawn as shapes so they join without gaps.
            lines = ""
            for offset, ch in enumerate(text):
                cx, cy = px + offset * CELL_W, top + y * CELL_H
                if ch in BLOCKS:
                    y0, y1 = BLOCKS[ch]
                    out.append(
                        f'    <rect x="{cx:.2f}" y="{cy + y0 * CELL_H:.2f}" '
                        f'width="{CELL_W + 0.3:.2f}" height="{(y1 - y0) * CELL_H + 0.3:.2f}" '
                        f'fill="{fg}"/>'
                    )
                elif ch in BOX_ARMS:
                    lines += box_path(ch, cx, cy)
            if lines:
                stroke = 2 if key[2] else 1.2
                out.append(
                    f'    <path d="{lines}" stroke="{fg}" stroke-width="{stroke}" '
                    'stroke-linecap="square" fill="none"/>'
                )
            visible = "".join(" " if ch in BLOCKS or ch in BOX_ARMS else ch for ch in text)
            if visible.strip():
                weight = ' font-weight="bold"' if key[2] else ""
                out.append(
                    f'    <text x="{px:.2f}" y="{baseline:.2f}" fill="{fg}"{weight} '
                    f'textLength="{span:.2f}" lengthAdjust="spacing">{escape(visible)}</text>'
                )
    out += ["  </g>", "</svg>", ""]
    return "\n".join(out)


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    hs = str(Path(sys.argv[1]).resolve())
    demo = Path(__file__).resolve().parent.parent / "docs" / "demo.sh"
    Path(sys.argv[2]).write_text(render(capture(hs, demo)), encoding="utf-8")


if __name__ == "__main__":
    main()
