#!/usr/bin/env python3
"""End-to-end native CLI checks, including assets, literal text and installed alias."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
BIN = ROOT / "target/release/ponysay"

def run(*args, input=None, env=None, binary=BIN):
    return subprocess.run([str(binary), *args], input=input, capture_output=True,
                          text=True, timeout=10, env={**os.environ, **(env or {})})

def success(*args, **kwargs):
    result = run(*args, **kwargs)
    assert result.returncode == 0, (args, result.stderr)
    assert not result.stderr, (args, result.stderr)
    return result.stdout

literal = r'''C:\Users\pony \- $HOME ${USER} `echo unsafe` "hi" 'hi' '''.strip()
out = success("-f", "twilight", "-Wn", "--no-color", "--", literal)
assert literal in out
assert "$$$" not in out and "$balloon" not in out
assert "first line" in success("-f", "pinkie", input="first line\nsecond line\n")
assert "世界 🌈" in success("-f", "twilight", "--no-color", "世界 🌈")
assert "Rust" in success("--version")
names = success("-l").splitlines()
assert "twilight" in names and "pinkie" in names and len(names) > 300
assert len(success("-A").splitlines()) >= len(names)
assert "ascii.say" in success("-B")
assert "NAME: Twilight" in success("-i", "-f", "twilight")
assert "NAME: Twilight" in success("+i", "-f", "twilight", "-Wn")
assert "< " not in success("-o", "-f", "twilight", "--no-color")
assert "\x1b" not in success("-f", "twilight", "hello", env={"NO_COLOR": "1"})
assert success("-q", "twilight")
assert success("-q")
assert success("-V", "-f", "twilight", "hello")
tty_plain = success("-V", "-f", "twilight", "--no-color", "hello")
assert "█" in tty_plain and len(tty_plain.splitlines()) > 20
assert tty_plain == success("-V", "-f", "twilight", "hello", env={"NO_COLOR": "1"})
assert tty_plain == success("-f", "twilight", "--no-color", "hello", env={"TERM": "linux"})
assert success("-of", "pinkie", "--no-color") == success("-o", "-f", "pinkie", "--no-color")
assert success("--ponies=twilight", "--", "hello") == success("-f", "twilight", "hello")
for style in success("-B").splitlines():
    success("-f", "twilight", "-b", style, "hello")
for args in [("-f", "does-not-exist"), ("--nonsense",), ("-W0", "hello"), ("-f",),
             ("--ponies=does-not-exist", "--", "hello"), ("--no-color=no", "hello"),
             ("-oZ",), ("-o界",)]:
    result = run(*args)
    assert result.returncode != 0 and result.stderr, args
with tempfile.TemporaryDirectory(prefix="ponysay-rust-test-") as tmp:
    tmp = Path(tmp)
    (tmp / "ponythink").symlink_to(BIN)
    thought = success("-f", "twilight", "--no-color", "thinking", binary=tmp / "ponythink")
    assert "( thinking" in thought and " o" in thought
    (tmp / "ponythink-rust").symlink_to(BIN)
    assert thought == success("-f", "twilight", "--no-color", "thinking", binary=tmp / "ponythink-rust")
    (tmp / "ponies").mkdir()
    (tmp / "ponies" / "custom.pony").write_text("$balloon$\n  $\\$\n  PONY\n")
    assert "PONY" in success("--data-dir", str(tmp), "-f", "custom", "hello")
    assert "PONY" in success("-f", str(tmp / "ponies/custom.pony"), "hello")
    (tmp / "ponies/别名.pony").symlink_to("custom.pony")
    (tmp / "quotes").mkdir()
    (tmp / "quotes/custom.1").write_text("A custom alias can quote.")
    assert "A custom alias can quote." in success("--data-dir", str(tmp), "-q", "别名")
    quoters = success("--data-dir", str(tmp), "--quoters").splitlines()
    assert "别名" in quoters and "custom" in quoters
    assert "PONY" in success("--data-dir", str(tmp), "-of别名", "--no-color")
    (tmp / "ponies/info.pony").write_text("$$$\nNAME: \x1b[31mRed\x1b[0m\n$$$\nPONY\n")
    assert "\x1b" not in success("--data-dir", str(tmp), "-i", "-f", "info", "--no-color")
if os.name == "posix":
    invalid = subprocess.run([os.fsencode(BIN), b"\xff"], capture_output=True, timeout=10)
    assert invalid.returncode == 2 and b"UTF-8" in invalid.stderr and b"panicked" not in invalid.stderr
print(f"CLI checks passed; {len(names)} normal pony names, all balloon styles.")
