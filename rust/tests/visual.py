#!/usr/bin/env python3
"""Render real PTY output for visual review. Run with: uv run --with pyte --with pillow python rust/tests/visual.py

These are terminal-emulator renderings of captured program output, not screenshots
of the user's Terminal application. Raw ANSI captures are saved beside each PNG.
"""
import fcntl
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import termios
import time

from PIL import Image, ImageDraw, ImageFont
import pyte

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "rust/visual-checks"
BIN = ROOT / "target/release/ponysay"
PALETTE = {"default": "#d7dae0", "black": "#000000", "red": "#cd0000", "green": "#00cd00",
           "brown": "#cdcd00", "blue": "#0000ee", "magenta": "#cd00cd", "cyan": "#00cdcd", "white": "#e5e5e5"}

def capture(args):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 60, 200, 0, 0))
    env = {**os.environ, "TERM": "xterm-256color", "PONYSAY_FULL_WIDTH": "yes"}
    env.pop("NO_COLOR", None)
    process = subprocess.Popen([str(BIN), *args], stdin=subprocess.DEVNULL,
                               stdout=slave, stderr=subprocess.PIPE, env=env)
    os.close(slave)
    chunks = []
    deadline = time.monotonic() + 10
    try:
        while time.monotonic() < deadline:
            readable, _, _ = select.select([master], [], [], 0.2)
            if not readable:
                if process.poll() is not None:
                    break
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError:
                break
            if not chunk:
                break
            chunks.append(chunk)
        process.wait(timeout=2)
        stderr = process.stderr.read()
        assert process.returncode == 0 and not stderr, stderr
    finally:
        os.close(master)
        if process.poll() is None:
            process.kill()
            process.wait()
    return b"".join(chunks)

def color(value, background=False):
    if value == "default":
        return "#11151c" if background else "#d7dae0"
    if value in PALETTE:
        return PALETTE[value]
    if len(value) == 6:
        return "#" + value
    return "#d7dae0"

def image(name, args, title):
    data = capture(args)
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{name}.ansi").write_bytes(data)
    screen = pyte.Screen(200, 80)
    pyte.Stream(screen).feed(data.decode())
    rows = max(i for i, row in enumerate(screen.display) if row.strip()) + 1
    cols = max(len(row.rstrip()) for row in screen.display)
    font = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 20)
    label = ImageFont.truetype("/System/Library/Fonts/Menlo.ttc", 17)
    cw, ch = 12, 24
    canvas = Image.new("RGB", (max(840, cols * cw + 48), rows * ch + 138), "#11151c")
    draw = ImageDraw.Draw(canvas)
    draw.text((24, 17), title, font=label, fill="#a4cfef")
    draw.text((24, 45), "Actual native stdout captured through a PTY", font=label, fill="#8b95a5")
    for y in range(rows):
        for x in range(cols):
            cell = screen.buffer[y][x]
            fg, bg = color(cell.fg), color(cell.bg, True)
            if cell.reverse:
                fg, bg = bg, fg
            draw.rectangle((24 + x*cw, 80 + y*ch, 24 + (x+1)*cw, 80 + (y+1)*ch), fill=bg)
            draw.text((24 + x*cw, 80 + y*ch), cell.data, font=font, fill=fg, anchor="la")
    draw.text((24, canvas.height-30), "Exit 0 | empty stderr | no Python runtime", font=label, fill="#8b95a5")
    canvas.save(OUT / f"{name}.png")
    print(OUT / f"{name}.png")

image("speech", ["-f", "twilight", "-Wn", "--", r"Rust works: C:\ponies \- $HOME"], "Speech bubble / literal unescaped characters")
image("thought", ["--think", "-f", "luna", "-W40", "Native Rust.\nThe night is so peaceful."], "Thought bubble / multiline message")
image("embedded", ["+f", "velvetremedy", "-Wn", "The embedded balloon works."], "Embedded balloon / original colored artwork")
