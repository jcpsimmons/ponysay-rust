#!/usr/bin/env python3
"""Compare complete native and original ponysay processes, with captured output."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--native", type=Path, default=ROOT / "target/release/ponysay")
    parser.add_argument("--runs", type=int, default=30)
    args = parser.parse_args()
    if args.runs < 3:
        parser.error("--runs must be at least 3")
    workloads = [
        ("shell greeting", ["-f", "twilight", "I know all your secrets, teehee!"], None),
        ("multiline stdin", ["-f", "pinkie"], "A native pony.\nBackslashes: C:\\ponies \\-\nUnicode: 世界 🌈\n"),
        ("wrapped paragraph", ["-f", "twilight", "-W", "65"], "Friendship is magic. " * 150),
    ]
    binaries = {"python_homebrew": args.baseline.resolve(), "rust": args.native.resolve()}
    env = {**os.environ, "PONYSAY_FULL_WIDTH": "yes"}
    results = []
    for name, flags, stdin in workloads:
        samples = {key: [] for key in binaries}
        stderr = {}
        for iteration in range(args.runs + 2):
            keys = list(binaries)
            if iteration % 2:
                keys.reverse()
            for key in keys:
                started = time.perf_counter()
                result = subprocess.run([str(binaries[key]), *flags], input=stdin,
                    capture_output=True, text=True, timeout=15, env=env)
                elapsed = time.perf_counter() - started
                if result.returncode or not result.stdout:
                    raise RuntimeError(f"{key}: exit {result.returncode}: {result.stderr}")
                if key == "rust" and result.stderr:
                    raise RuntimeError(f"Rust wrote to stderr: {result.stderr}")
                stderr[key] = bool(result.stderr)
                if iteration >= 2:
                    samples[key].append(elapsed * 1000)
        medians = {key: statistics.median(value) for key, value in samples.items()}
        speedup = medians["python_homebrew"] / medians["rust"]
        results.append({"name": name, "flags": flags, "stdin_bytes": len((stdin or "").encode()),
                        "samples_ms": samples, "median_ms": medians, "speedup": speedup,
                        "baseline_warning": stderr["python_homebrew"], "native_stderr_empty": not stderr["rust"]})
        print(f"{name}: Python {medians['python_homebrew']:.2f} ms; Rust {medians['rust']:.2f} ms; {speedup:.1f}x")
    data = {"measured_at": datetime.now(timezone.utc).isoformat(), "os": f"{platform.system()} {platform.release()}",
            "machine": platform.machine(), "runs": args.runs, "warmups": 2,
            "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "baseline_version": "Homebrew ponysay 3.0.3_7 / Python 3.14.7", "results": results,
            "method": "Sequential alternating complete process wall times, stdout/stderr captured; warm cache. No claim of identical wrapping bytes."}
    out = ROOT / "rust/benchmarks/startup.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(data, indent=2) + "\n")

if __name__ == "__main__":
    main()
