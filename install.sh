#!/bin/sh
# MoodCut installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/hamzarasheed309/MoodCut/HEAD/install.sh | sh
#
# Linux x64: downloads the latest release into ~/.local/bin.
# Elsewhere (or with MOODCUT_FROM_SOURCE=1): builds from source with cargo and npm.
# Then installs FFmpeg if it's missing (portable build on Linux) and connects
# the AI apps found on this computer (skip with MOODCUT_NO_AI=1).
set -eu

REPO="hamzarasheed309/MoodCut"
BIN_DIR="${MOODCUT_BIN_DIR:-$HOME/.local/bin}"
say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

mkdir -p "$BIN_DIR"
os="$(uname -s)"
arch="$(uname -m)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say ""
say "  MoodCut - the AI-native video editor"
say ""

installed=""
prebuilt_broken=""
if [ "$os" = "Linux" ] && [ "$arch" = "x86_64" ] && [ -z "${MOODCUT_FROM_SOURCE:-}" ]; then
  url="https://github.com/$REPO/releases/latest/download/moodcut-linux-x64.tar.gz"
  say "Downloading $url"
  if curl -fsSL "$url" -o "$tmp/moodcut.tar.gz" && tar -xzf "$tmp/moodcut.tar.gz" -C "$tmp"; then
    # the release is built on a recent Ubuntu; check it runs here before installing it
    if "$tmp/moodcut" --version >/dev/null 2>&1; then
      install -m 755 "$tmp/moodcut" "$BIN_DIR/moodcut"
      installed=1
    else
      prebuilt_broken=1
      say "The prebuilt MoodCut doesn't run on this system (its C library is older than the build's); building from source instead."
    fi
  else
    say "No prebuilt release found; building from source instead."
  fi
fi

if [ -z "$installed" ]; then
  if [ -n "${prebuilt_broken:-}" ] && ! command -v cargo >/dev/null 2>&1; then
    die "the prebuilt binary needs a newer Linux (glibc 2.39, e.g. Ubuntu 24.04+). Install Rust (https://rustup.rs) and Node.js 20+, then run this again to build from source"
  fi
  command -v cargo >/dev/null 2>&1 || die "building needs Rust (https://rustup.rs) and Node.js 20+"
  command -v npm >/dev/null 2>&1 || die "building needs Node.js 20+ (https://nodejs.org)"
  command -v git >/dev/null 2>&1 || die "building needs git"
  say "Building MoodCut from source (a few minutes)..."
  git clone --depth 1 "https://github.com/$REPO" "$tmp/src"
  (cd "$tmp/src/gui" && npm ci --silent && npm run build --silent)
  (cd "$tmp/src" && cargo build --release --quiet)
  install -m 755 "$tmp/src/target/release/moodcut" "$BIN_DIR/moodcut"
fi

mc="$BIN_DIR/moodcut"
say "Installed $("$mc" --version) to $mc"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "Add $BIN_DIR to your PATH so AI apps and terminals find moodcut:"
     say "  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.profile" ;;
esac

if ! command -v ffmpeg >/dev/null 2>&1; then
  if [ "$os" = "Darwin" ]; then
    say "FFmpeg is needed for video: brew install ffmpeg"
  else
    say "Installing a portable FFmpeg (no root needed)..."
    "$mc" setup || say "FFmpeg install failed; install it with your package manager (e.g. sudo apt install ffmpeg)"
  fi
fi

if [ -z "${MOODCUT_NO_AI:-}" ]; then
  "$mc" install-ai || true
fi
say ""
say "Done. Start the editor with: moodcut    (claude.ai / ChatGPT on the web: moodcut connect)"
