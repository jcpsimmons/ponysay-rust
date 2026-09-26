#!/usr/bin/env python3
"""Compare native visible output with the retained upstream Python renderer.

Run after `cargo build --release --locked`:

    python3 rust/tests/render_parity.py
    python3 rust/tests/render_parity.py --include-tty

The default suite covers every bundled xterm pony path (including symlinks),
every custom style with top and bottom balloons, and ordinary default wrapping.
ANSI encodings and the final newline are normalized; visible characters and
spaces must match exactly. Unicode width fixes, minimum-height fixes, and the
native renderer's conventional long-word wrapping are tested by Rust unit tests
instead of requiring compatibility with those upstream bugs/quirks. The optional
TTY suite records upstream failures in twilightrage's palette-reset trailer and
velvetremedy's embedded-balloon palette handling, using their corresponding xterm
assets as visible-art oracles for those two cases.

No external dependencies or installed Python ponysay package are required.
Python is a test oracle here, never a runtime dependency of the native program.
"""

import argparse
import difflib
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "src"))

import backend as upstream  # noqa: E402
from balloon import Balloon  # noqa: E402

# Metadata diagnostics on fd 3 are outside the renderer's visible output.
upstream.printinfo = lambda *_args, **_kwargs: None


def visible(text):
    """Remove ANSI CSI, OSC, Linux-VT palette, and string-control sequences."""
    result = []
    i = 0
    while i < len(text):
        if text[i] != "\x1b":
            result.append(text[i])
            i += 1
            continue
        if i + 1 >= len(text):
            break
        kind = text[i + 1]
        i += 2
        if kind == "[":
            while i < len(text):
                char = text[i]
                i += 1
                if "@" <= char <= "~":
                    break
        elif kind == "]" and text[i:i + 1] == "P":
            i += 8  # P plus exactly seven hexadecimal palette digits.
        elif kind == "]" and text[i:i + 1] == "R":
            i += 1
        elif kind in "]P_^":
            while i < len(text):
                if text[i] == "\x07":
                    i += 1
                    break
                if text[i:i + 2] == "\x1b\\":
                    i += 2
                    break
                i += 1
    return "".join(result).rstrip("\n")


def expected(pony, message, *, style=None, think=False, wrap=None):
    renderer = upstream.Backend(
        message=message,
        ponyfile=str(pony),
        wrapcolumn=wrap,
        width=None,
        balloon=Balloon.fromFile(str(style) if style else None, think),
        hyphen="-",
        linkcolour="",
        ballooncolour="",
        mode="",
        infolevel=0,
    )
    renderer.parse()
    return visible(renderer.output)


def native(binary, pony, message, *, style=None, think=False, wrap=None):
    args = [str(binary), "-f", str(pony), "--no-color"]
    if wrap is None:
        args.append("-Wn")
    elif wrap != 65:
        args.extend(["-W", str(wrap)])
    if style:
        args.extend(["-b", str(style)])
    if think:
        args.append("--think")
    args.extend(["--", message])
    environment = dict(os.environ)
    environment.update(TERM="xterm-256color", PONYSAY_FULL_WIDTH="yes")
    environment.pop("PONYSAY_WRAP_LIMIT", None)
    environment.pop("PONYSAY_WRAP_EXCEED", None)
    environment.pop("PONYSAY_WRAP_HYPHEN", None)
    process = subprocess.run(args, text=True, capture_output=True,
                             env=environment, timeout=10, check=False)
    if process.returncode or process.stderr:
        raise AssertionError(
            f"native process failed ({process.returncode}): {process.stderr.strip()}"
        )
    if "\x1b" in process.stdout:
        raise AssertionError("--no-color output still contains ANSI escapes")
    return visible(process.stdout)


def cases(include_tty):
    groups = ["ponies", "extraponies"]
    if include_tty:
        groups.extend(["ttyponies", "extrattyponies"])
    for group in groups:
        for pony in sorted((ROOT / group).glob("*.pony")):
            yield f"corpus/{group}/{pony.name}", pony, "Rust keeps the ponies.", {}

    anchors = [ROOT / "ponies/twilight.pony", ROOT / "ponies/twilightrage.pony"]
    for style in sorted((ROOT / "balloons").iterdir()):
        if not style.is_file() or style.suffix not in (".say", ".think"):
            continue
        for pony in anchors:
            for message in ["Friendship is magic!", "First line\nSecond line"]:
                yield (
                    f"style/{style.name}/{pony.stem}/{message.count(chr(10)) + 1}lines",
                    pony,
                    message,
                    {"style": style, "think": style.suffix == ".think"},
                )

    messages = [
        "Friendship is magic!",
        "The quick brown fox jumps over the lazy dog and has a very long conversation about ponies.",
        "A kind word can make a day feel bright. We hope that you enjoy this small herd of ponies.",
        "First line\nSecond line has a few more words and then a little more to say before we finish the sentence.",
        r'''Literal text: C:\ponies\new \- $HOME ${USER} "quotes" and 'single quotes'.''',
    ]
    for pony in anchors:
        for think in (False, True):
            for number, message in enumerate(messages, start=1):
                yield (
                    f"default-wrap/{pony.stem}/{'think' if think else 'say'}/{number}",
                    pony,
                    message,
                    {"think": think, "wrap": 65},
                )


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/ponysay")
    parser.add_argument("--include-tty", action="store_true",
                        help="also compare every Linux-console pony path")
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"{binary} is missing; run cargo build --release --locked first")
    # Upstream wrapping reads these process variables directly.
    for variable in ("PONYSAY_WRAP_LIMIT", "PONYSAY_WRAP_EXCEED", "PONYSAY_WRAP_HYPHEN"):
        os.environ.pop(variable, None)

    start = time.monotonic()
    failures = []
    oracle_fallbacks = []
    count = 0
    for label, pony, message, options in cases(args.include_tty):
        count += 1
        try:
            try:
                original = expected(pony, message, **options)
            except TypeError as error:
                if (label != "corpus/ttyponies/twilightrage.pony"
                        or "NoneType" not in str(error)):
                    raise
                # Upstream's bottom-balloon loop crashes when this VT asset's
                # palette-reset trailer follows its final newline. Its xterm
                # twin has the same glyphs, and provides an independent oracle.
                original = expected(ROOT / "ponies/twilightrage.pony", message, **options)
                oracle_fallbacks.append(label)
            if label == "corpus/extrattyponies/velvetremedy.pony" and "]P" in original:
                # The Python overlay loop loses the ESC bytes from palette
                # sequences, leaking ]Pxxxxxxx and counting them as artwork.
                original = expected(ROOT / "extraponies/velvetremedy.pony", message, **options)
                oracle_fallbacks.append(label)
            rewritten = native(binary, pony, message, **options)
            if original != rewritten:
                diff = "\n".join(list(difflib.unified_diff(
                    original.splitlines(), rewritten.splitlines(),
                    fromfile="upstream Python", tofile="native Rust", lineterm="",
                ))[:28])
                raise AssertionError(diff)
        except Exception as error:
            failures.append(label)
            if len(failures) <= 5:
                print(f"FAIL {label}\n{error}", file=sys.stderr, flush=True)
        if count % 200 == 0:
            print(f"Compared {count} cases; {len(failures)} differences", flush=True)

    elapsed = time.monotonic() - start
    print(f"Renderer checks: {count - len(failures)}/{count} cases passed in {elapsed:.2f}s.")
    if failures:
        print("Failed cases: " + ", ".join(failures), file=sys.stderr)
        return 1
    if oracle_fallbacks:
        print("Known upstream TTY regressions passed using equivalent xterm art: "
              + ", ".join(oracle_fallbacks))
    print(f"{count - len(oracle_fallbacks)} cases directly match upstream visible artwork, balloon borders, and ordinary wrapping.")
    print("Intentional differences in Unicode width, minimum height, and long-word wrapping remain covered by Rust tests.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
