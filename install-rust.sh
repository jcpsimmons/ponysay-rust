#!/bin/sh
# Build the native fork and install it without Homebrew or Python.
# Usage: ./install-rust.sh [prefix]    (default: $HOME/.local)
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
prefix=${1:-"$HOME/.local"}
build_dir=${CARGO_TARGET_DIR:-target}
cargo build --release --locked --target-dir "$build_dir"
mkdir -p "$prefix/bin"
for name in ponysay-rust ponysay ponythink; do
  if [ -d "$prefix/bin/$name" ] && [ ! -L "$prefix/bin/$name" ]; then
    printf 'Cannot replace directory: %s/bin/%s\n' "$prefix" "$name" >&2
    exit 1
  fi
done
if [ -L "$prefix/bin/ponysay-rust" ]; then
  rm "$prefix/bin/ponysay-rust"
fi
install -m 755 "$build_dir/release/ponysay" "$prefix/bin/ponysay-rust"
ln -sfn ponysay-rust "$prefix/bin/ponysay"
ln -sfn ponysay-rust "$prefix/bin/ponythink"
"$prefix/bin/ponysay" --version
printf 'Installed ponysay-rust, ponysay and ponythink in %s/bin\n' "$prefix"
printf 'Put this directory before Homebrew in PATH.\n'
