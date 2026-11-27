#!/usr/bin/env python3
"""Record a reproducible animated demo (GIF + MP4) of hs in a real pseudo-terminal.

Uses only Python's standard library, `rsvg-convert`, and `ffmpeg`, rendering
frames in the trivial.si color palette against a disposable temporary database.

Usage:
    python3 scripts/record-demo-gif.py target/release/hs assets/demo-tui.gif [assets/demo-tui.mp4]
"""

import codecs
from dataclasses import dataclass
from datetime import date, timedelta
import fcntl
from html import escape
import os
from pathlib import Path
import pty
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time

COLS, ROWS = 100, 30
FONT_SIZE = 14
CELL_W = FONT_SIZE * 0.602  # DejaVu Sans Mono advance width
CELL_H = 18
PAD = 20
BAR = 32

BACKGROUND = "#152a40"
FOREGROUND = "#f7f8f5"

ANSI_COLORS = [
    "#152a40",  # 0 black
    "#ff622e",  # 1 red
    "#8fd18b",  # 2 green
    "#ffb35c",  # 3 yellow
    "#7aa7d9",  # 4 blue
    "#d29be0",  # 5 magenta
    "#6cc7e0",  # 6 cyan
    "#c7ced4",  # 7 white
    "#6b7580",  # 8 bright black
    "#ff8a5f",  # 9 bright red
    "#aee3a8",  # 10 bright green
    "#ffcf8a",  # 11 bright yellow
    "#a3c4ea",  # 12 bright blue
    "#e3bdec",  # 13 bright magenta
    "#a2dcec",  # 14 bright cyan
    "#ffffff",  # 15 bright white
]

BOX_ARMS = {
    "\u2500": "lr", "\u2502": "ud", "\u250c": "rd", "\u2510": "ld",
    "\u2514": "ru", "\u2518": "lu", "\u256d": "rd", "\u256e": "ld",
    "\u2570": "ru", "\u256f": "lu", "\u251c": "rud", "\u2524": "lud",
    "\u252c": "lrd", "\u2534": "lru", "\u253c": "lrud",
}

BLOCKS = {"\u2588": (0, 1), "\u2580": (0, 0.5), "\u2584": (0.5, 1)}


def xterm_256(idx: int) -> str:
    if 0 <= idx < 16:
        return ANSI_COLORS[idx]
    if 16 <= idx < 232:
        n = idx - 16
        r, g, b = n // 36, (n // 6) % 6, n % 6
        lut = [0, 95, 135, 175, 215, 255]
        return f"#{lut[r]:02x}{lut[g]:02x}{lut[b]:02x}"
    if 232 <= idx < 256:
        v = 8 + (idx - 232) * 10
        return f"#{v:02x}{v:02x}{v:02x}"
    return FOREGROUND


@dataclass
class Cell:
    ch: str = " "
    fg: str = FOREGROUND
    bg: str = BACKGROUND
    bold: bool = False
    dim: bool = False
    reverse: bool = False


class Screen:
    """Minimal VT100/ANSI screen buffer supporting normal + alternate screens."""

    def __init__(self, cols: int, rows: int) -> None:
        self.cols = cols
        self.rows = rows
        self.main = self._blank()
        self.alt = self._blank()
        self.in_alt = False
        self.row = 0
        self.col = 0
        self.saved_main_pos = (0, 0)
        self.fg = FOREGROUND
        self.bg = BACKGROUND
        self.bold = False
        self.dim = False
        self.reverse = False
        self._buf = ""

    def _blank(self) -> list[list[Cell]]:
        return [[Cell() for _ in range(self.cols)] for _ in range(self.rows)]

    @property
    def grid(self) -> list[list[Cell]]:
        return self.alt if self.in_alt else self.main

    def reset_style(self) -> None:
        self.fg = FOREGROUND
        self.bg = BACKGROUND
        self.bold = False
        self.dim = False
        self.reverse = False

    def _newline(self) -> None:
        self.row += 1
        if self.row >= self.rows:
            self.grid.pop(0)
            self.grid.append([Cell() for _ in range(self.cols)])
            self.row = self.rows - 1

    def _apply_sgr(self, params: list[int]) -> None:
        if not params:
            self.reset_style()
            return
        i = 0
        while i < len(params):
            code = params[i]
            if code == 0:
                self.reset_style()
            elif code == 1:
                self.bold = True
            elif code == 2:
                self.dim = True
            elif code == 7:
                self.reverse = True
            elif code == 22:
                self.bold = False
                self.dim = False
            elif code == 27:
                self.reverse = False
            elif 30 <= code <= 37:
                self.fg = ANSI_COLORS[code - 30]
            elif code == 39:
                self.fg = FOREGROUND
            elif 40 <= code <= 47:
                self.bg = ANSI_COLORS[code - 40]
            elif code == 49:
                self.bg = BACKGROUND
            elif 90 <= code <= 97:
                self.fg = ANSI_COLORS[8 + (code - 90)]
            elif 100 <= code <= 107:
                self.bg = ANSI_COLORS[8 + (code - 100)]
            elif code in (38, 48) and i + 1 < len(params):
                is_fg = code == 38
                mode = params[i + 1]
                if mode == 5 and i + 2 < len(params):
                    c = xterm_256(params[i + 2])
                    if is_fg:
                        self.fg = c
                    else:
                        self.bg = c
                    i += 2
                elif mode == 2 and i + 4 < len(params):
                    r, g, b = params[i + 2], params[i + 3], params[i + 4]
                    c = f"#{r:02x}{g:02x}{b:02x}"
                    if is_fg:
                        self.fg = c
                    else:
                        self.bg = c
                    i += 4
            i += 1

    def feed(self, text: str) -> None:
        s = self._buf + text
        self._buf = ""
        i = 0
        n = len(s)
        while i < n:
            ch = s[i]
            if ch == "\x1b":
                if i + 1 >= n:
                    self._buf = s[i:]
                    return
                nxt = s[i + 1]
                if nxt == "[":
                    j = i + 2
                    while j < n and not ("@" <= s[j] <= "~"):
                        j += 1
                    if j >= n:
                        self._buf = s[i:]
                        return
                    raw_params = s[i + 2 : j]
                    final = s[j]
                    self._handle_csi(raw_params, final)
                    i = j + 1
                    continue
                if nxt == "]":
                    # OSC sequence terminated by BEL or ST
                    bel = s.find("\x07", i + 2)
                    st = s.find("\x1b\\", i + 2)
                    if bel == -1 and st == -1:
                        self._buf = s[i:]
                        return
                    if bel != -1 and (st == -1 or bel < st):
                        i = bel + 1
                    else:
                        i = st + 2
                    continue
                i += 2
                continue
            if ch == "\r":
                self.col = 0
            elif ch == "\n":
                self._newline()
            elif ch == "\b":
                self.col = max(0, self.col - 1)
            elif ch == "\t":
                self.col = min(self.cols - 1, ((self.col // 8) + 1) * 8)
            elif ch >= " ":
                if self.col >= self.cols:
                    self.col = 0
                    self._newline()
                self.grid[self.row][self.col] = Cell(
                    ch=ch,
                    fg=self.fg,
                    bg=self.bg,
                    bold=self.bold,
                    dim=self.dim,
                    reverse=self.reverse,
                )
                self.col += 1
            i += 1

    def _handle_csi(self, raw_params: str, final: str) -> None:
        if raw_params.startswith("?"):
            mode = raw_params[1:]
            if mode in ("1049", "47", "1047"):
                if final == "h":
                    self.saved_main_pos = (self.row, self.col)
                    self.in_alt = True
                    self.alt = self._blank()
                    self.row, self.col = 0, 0
                elif final == "l":
                    self.in_alt = False
                    self.row, self.col = self.saved_main_pos
            return
        parts = [int(p) if p.isdigit() else 0 for p in raw_params.split(";")] if raw_params else []
        if final == "m":
            self._apply_sgr(parts)
        elif final in ("H", "f"):
            r = parts[0] if len(parts) >= 1 and parts[0] > 0 else 1
            c = parts[1] if len(parts) >= 2 and parts[1] > 0 else 1
            self.row = min(self.rows - 1, max(0, r - 1))
            self.col = min(self.cols - 1, max(0, c - 1))
        elif final == "A":
            self.row = max(0, self.row - max(1, parts[0] if parts else 1))
        elif final == "B":
            self.row = min(self.rows - 1, self.row + max(1, parts[0] if parts else 1))
        elif final == "C":
            self.col = min(self.cols - 1, self.col + max(1, parts[0] if parts else 1))
        elif final == "D":
            self.col = max(0, self.col - max(1, parts[0] if parts else 1))
        elif final == "G":
            c = parts[0] if parts and parts[0] > 0 else 1
            self.col = min(self.cols - 1, max(0, c - 1))
        elif final == "J":
            kind = parts[0] if parts else 0
            if kind in (2, 3):
                if self.in_alt:
                    self.alt = self._blank()
                else:
                    self.main = self._blank()
                self.row, self.col = 0, 0
            elif kind == 0:
                for c in range(self.col, self.cols):
                    self.grid[self.row][c] = Cell()
                for r in range(self.row + 1, self.rows):
                    self.grid[r] = [Cell() for _ in range(self.cols)]
        elif final == "K":
            kind = parts[0] if parts else 0
            if kind == 0:
                for c in range(self.col, self.cols):
                    self.grid[self.row][c] = Cell()
            elif kind == 1:
                for c in range(0, self.col + 1):
                    self.grid[self.row][c] = Cell()
            elif kind == 2:
                self.grid[self.row] = [Cell() for _ in range(self.cols)]

    def snapshot(self) -> list[list[Cell]]:
        return [
            [Cell(c.ch, c.fg, c.bg, c.bold, c.dim, c.reverse) for c in row]
            for row in self.grid
        ]

    def plain_text(self) -> str:
        return "\n".join("".join(c.ch for c in row) for row in self.grid)


def box_path(ch: str, x: float, y: float) -> str:
    cx, cy = x + CELL_W / 2, y + CELL_H / 2
    ends = {"l": (x, cy), "r": (x + CELL_W, cy), "u": (cx, y), "d": (cx, y + CELL_H)}
    return "".join(
        f"M{cx:.2f} {cy:.2f}L{ends[arm][0]:.2f} {ends[arm][1]:.2f}"
        for arm in BOX_ARMS[ch]
    )


def render_svg(rows: list[list[Cell]], title: str) -> str:
    width = PAD * 2 + COLS * CELL_W
    height = BAR + PAD * 2 + ROWS * CELL_H
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0f}" height="{height:.0f}" '
        f'viewBox="0 0 {width:.0f} {height:.0f}">',
        f'  <rect width="{width:.0f}" height="{height:.0f}" fill="{BACKGROUND}"/>',
        f'  <path d="M0 {BAR} H{width:.0f}" stroke="#2a4058" stroke-width="1"/>',
        '  <circle cx="20" cy="16" r="5.5" fill="#ff622e"/>',
        '  <circle cx="38" cy="16" r="5.5" fill="#6b7580"/>',
        '  <circle cx="56" cy="16" r="5.5" fill="#6b7580"/>',
        f'  <text x="{width / 2:.0f}" y="21" font-family="Roboto, Arial, sans-serif" '
        f'font-size="13" fill="#c7ced4" text-anchor="middle">{escape(title)}</text>',
        f'  <g font-family="DejaVu Sans Mono, Liberation Mono, monospace" '
        f'font-size="{FONT_SIZE}" xml:space="preserve">',
    ]
    top = BAR + PAD
    for y, row in enumerate(rows):
        baseline = top + y * CELL_H + CELL_H * 0.76
        x = 0
        while x < COLS:
            cell = row[x]
            fg = "#8b98a6" if cell.dim and cell.fg == FOREGROUND else cell.fg
            bg = cell.bg
            if cell.reverse:
                fg, bg = bg, fg
            key = (fg, bg, cell.bold)
            start = x
            text = ""
            while x < COLS:
                c = row[x]
                cfg = "#8b98a6" if c.dim and c.fg == FOREGROUND else c.fg
                cbg = c.bg
                if c.reverse:
                    cfg, cbg = cbg, cfg
                if (cfg, cbg, c.bold) != key:
                    break
                text += c.ch
                x += 1
            px = PAD + start * CELL_W
            span = (x - start) * CELL_W
            if bg != BACKGROUND:
                out.append(
                    f'    <rect x="{px:.2f}" y="{top + y * CELL_H}" '
                    f'width="{span + 0.4:.2f}" height="{CELL_H}" fill="{bg}"/>'
                )
            lines = ""
            for offset, ch in enumerate(text):
                cx, cy = px + offset * CELL_W, top + y * CELL_H
                if ch in BLOCKS:
                    y0, y1 = BLOCKS[ch]
                    out.append(
                        f'    <rect x="{cx:.2f}" y="{cy + y0 * CELL_H:.2f}" '
                        f'width="{CELL_W + 0.35:.2f}" height="{(y1 - y0) * CELL_H + 0.35:.2f}" '
                        f'fill="{fg}"/>'
                    )
                elif ch in BOX_ARMS:
                    lines += box_path(ch, cx, cy)
            if lines:
                stroke = 2.0 if key[2] else 1.2
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


def run_cli(hs: str, db: str, env: dict[str, str], args: list[str]) -> str:
    proc = subprocess.run(
        [hs, "--db", db, *args],
        env=env,
        text=True,
        capture_output=True,
        check=True,
    )
    return proc.stdout


def main() -> None:
    if len(sys.argv) not in (3, 4):
        raise SystemExit(__doc__)
    hs = str(Path(sys.argv[1]).resolve())
    gif_out = Path(sys.argv[2]).resolve()
    mp4_out = Path(sys.argv[3]).resolve() if len(sys.argv) == 4 else None

    frames: list[tuple[list[list[Cell]], str, float]] = []

    def add_frame(screen: Screen, title: str, duration: float) -> None:
        frames.append((screen.snapshot(), title, duration))

    def type_command(screen: Screen, cmd: str, title: str) -> None:
        screen.feed("\x1b[1;31m$\x1b[0m ")
        add_frame(screen, title, 0.25)
        chunk = 4
        for i in range(0, len(cmd), chunk):
            screen.feed(cmd[i : i + chunk])
            add_frame(screen, title, 0.07)
        screen.feed("\r\n")

    with tempfile.TemporaryDirectory(prefix="hs-gif-") as tmp:
        tmp_path = Path(tmp)
        db = str(tmp_path / "demo.db")
        env = dict(os.environ)
        env["XDG_DATA_HOME"] = str(tmp_path / "data")
        env["TERM"] = "xterm-256color"
        env["COLUMNS"] = str(COLS)
        env.pop("NO_COLOR", None)

        due = (date.today() + timedelta(days=3)).isoformat()
        screen = Screen(COLS, ROWS)

        # Pre-seed four fictional background tasks so the matrix is rich immediately.
        for args in [
            ["task", "add", "Outline the Kestrel Observatory talk", "--quadrant", "plan", "--category", "Work", "--tags", "writing"],
            ["task", "add", "Book the Harbor Street meeting room", "--quadrant", "delegate", "--category", "Work"],
            ["task", "add", "Sort the 2019 photo archive", "--quadrant", "drop", "--category", "Home"],
            ["task", "add", "Read the Orchard Lane field notes"],
        ]:
            run_cli(hs, db, env, args)

        # Scene 1: Add the urgent task and view the ASCII Eisenhower matrix.
        cmd_add = f"hs task add 'Send the Lumen Bakery menu update' --time-label Writing --category Work --quadrant do --due {due}"
        type_command(screen, cmd_add, "hs: terminal time tracker")
        out = run_cli(
            hs, db, env,
            ["task", "add", "Send the Lumen Bakery menu update", "--time-label", "Writing", "--category", "Work", "--tags", "client", "--quadrant", "do", "--due", due],
        )
        screen.feed(out.replace("\n", "\r\n") + "\r\n")
        add_frame(screen, "hs: terminal time tracker", 0.9)

        type_command(screen, "hs matrix", "hs matrix")
        out = run_cli(hs, db, env, ["matrix"])
        screen.feed(out.replace("\n", "\r\n") + "\r\n")
        add_frame(screen, "hs matrix", 2.4)

        # Scene 2: Open `hs tui` in a real PTY, start focus, navigate, inspect, quit.
        type_command(screen, "hs tui", "hs tui")
        add_frame(screen, "hs tui", 0.35)

        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe(hs, [hs, "--db", db, "tui", "--no-sound"], env)

        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        decoder = codecs.getincrementaldecoder("utf-8")("replace")

        def pump(seconds: float) -> None:
            end = time.time() + seconds
            while time.time() < end:
                ready, _, _ = select.select([fd], [], [], 0.04)
                if ready:
                    try:
                        data = os.read(fd, 65536)
                        if not data:
                            screen.feed(decoder.decode(b"", final=True))
                            return
                        screen.feed(decoder.decode(data))
                    except OSError:
                        screen.feed(decoder.decode(b"", final=True))
                        return

        def wait_for(needle: str, timeout: float = 10.0) -> None:
            end = time.time() + timeout
            while time.time() < end:
                pump(0.15)
                if needle in screen.plain_text():
                    return
            raise SystemExit(f"timed out waiting for {needle!r}")

        wait_for("Lumen Bakery")
        pump(0.25)
        add_frame(screen, "hs tui", 1.1)

        # Select Do pane (key `1`) and press Space to start focus session!
        os.write(fd, b"1")
        pump(0.25)
        add_frame(screen, "hs tui", 0.5)

        os.write(fd, b" ")
        wait_for("FOCUS RUNNING")
        pump(0.25)
        add_frame(screen, "hs tui: Pomodoro focus", 1.0)

        # Let the big live Pomodoro countdown tick twice visibly.
        for _ in range(2):
            pump(1.05)
            add_frame(screen, "hs tui: Pomodoro focus", 1.0)

        # Open task details (`Enter`), view briefly, close (`Esc`).
        os.write(fd, b"\r")
        pump(0.35)
        add_frame(screen, "hs tui: task details", 1.6)
        os.write(fd, b"\x1b")
        pump(0.3)
        add_frame(screen, "hs tui: Pomodoro focus", 0.5)

        # Move across panes with arrow keys (Left -> Plan, Down -> Drop, Down -> Inbox).
        for key in (b"\x1b[D", b"\x1b[B", b"\x1b[B"):
            os.write(fd, key)
            pump(0.3)
            add_frame(screen, "hs tui: Eisenhower matrix", 0.65)

        # Pause with Space, then resume with Space.
        os.write(fd, b" ")
        wait_for("PAUSED")
        pump(0.25)
        add_frame(screen, "hs tui: paused", 1.0)

        os.write(fd, b" ")
        wait_for("FOCUS RUNNING")
        pump(0.25)
        add_frame(screen, "hs tui: resumed", 0.9)

        # Quit TUI — timer remains saved and running!
        os.write(fd, b"q")
        pump(0.6)
        os.waitpid(pid, 0)
        os.close(fd)

        # Scene 3: Show `hs pomo status` proving timer survived closing TUI.
        screen.feed("\x1b[2J\x1b[H")
        type_command(screen, "hs pomo status", "hs pomo status: timer persists")
        out = run_cli(hs, db, env, ["pomo", "status"])
        screen.feed(out.replace("\n", "\r\n") + "\r\n")
        add_frame(screen, "hs pomo status: timer persists", 2.8)

        # Cleanly stop the demo focus session.
        run_cli(hs, db, env, ["pomo", "stop"])

        # Render SVG -> PNG and encode GIF (+ optional MP4).
        frames_dir = tmp_path / "frames"
        frames_dir.mkdir()
        concat_lines = []
        for idx, (grid, title, duration) in enumerate(frames):
            svg_path = frames_dir / f"frame_{idx:04d}.svg"
            png_path = frames_dir / f"frame_{idx:04d}.png"
            svg_path.write_text(render_svg(grid, title), encoding="utf-8")
            subprocess.run(
                ["rsvg-convert", "-z", "1.5", str(svg_path), "-o", str(png_path)],
                check=True,
            )
            concat_lines.append(f"file '{png_path}'\nduration {duration:.3f}\n")
        # ffmpeg concat demuxer needs the final frame repeated once.
        last_png = frames_dir / f"frame_{len(frames) - 1:04d}.png"
        concat_lines.append(f"file '{last_png}'\n")
        concat_file = frames_dir / "frames.txt"
        concat_file.write_text("".join(concat_lines), encoding="utf-8")

        palette = frames_dir / "palette.png"
        subprocess.run(
            [
                "ffmpeg", "-y", "-v", "error",
                "-f", "concat", "-safe", "0", "-i", str(concat_file),
                "-vf", "palettegen=max_colors=64:stats_mode=diff",
                str(palette),
            ],
            check=True,
        )
        subprocess.run(
            [
                "ffmpeg", "-y", "-v", "error",
                "-f", "concat", "-safe", "0", "-i", str(concat_file),
                "-i", str(palette),
                "-lavfi", "paletteuse=dither=none",
                "-loop", "0",
                str(gif_out),
            ],
            check=True,
        )

        if mp4_out is not None:
            subprocess.run(
                [
                    "ffmpeg", "-y", "-v", "error",
                    "-f", "concat", "-safe", "0", "-i", str(concat_file),
                    "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:color=#152a40",
                    "-c:v", "libx264", "-pix_fmt", "yuv420p",
                    "-movflags", "+faststart", "-crf", "22", "-r", "24",
                    str(mp4_out),
                ],
                check=True,
            )

    total_sec = sum(d for _, _, d in frames)
    print(f"Wrote {gif_out} ({len(frames)} frames, {total_sec:.1f}s)")
    if mp4_out is not None:
        print(f"Wrote {mp4_out}")


if __name__ == "__main__":
    main()
