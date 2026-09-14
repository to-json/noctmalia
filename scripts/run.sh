#!/usr/bin/env bash
# Build and run the app.
#
# Both happen inside the devshell, which is what puts nix's Mesa (and so the hardware GL driver) on
# the library path. Running a devshell-built binary outside the shell does not work: it links nix's
# glibc and loader and cannot find the system's libraries at all.
#
#   scripts/run.sh              release — the only profile worth judging the UI on
#   scripts/run.sh --debug      dev profile, for a backtrace
#   scripts/run.sh --chrome     release with `wayland-chrome`: the shadow and rounded corners sit
#                               outside the xdg window geometry, so tiling and snapping see the real
#                               window. Needs noctalia-iced's patched winit/iced crates
#                               (third_party/patch.toml); Cargo.lock is restored afterwards.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile=release
chrome=0
case "${1:-}" in
  --debug) profile=debug; shift ;;
  --chrome) chrome=1; shift ;;
esac

# This machine's GPU (Intel HD 4000, Ivy Bridge/Gen7) renders iced incorrectly on both hardware
# Mesa paths: the GL driver (crocus) presents torn frames where only the last redrawn rectangle
# survives and the rest goes black, and Mesa's own Vulkan driver prints "Ivy Bridge Vulkan support
# is incomplete". Software rendering is correct, holds 60fps on this UI, and costs nothing while
# idle because the app only redraws on demand. noctalia-iced's clock demo shows the same corruption,
# so this is the stack on this hardware, not this application.
#
# Set NOCTMALIA_GPU=1 to use the GPU — do that on any machine newer than this one.
if [ "${NOCTMALIA_GPU:-0}" = 0 ]; then
  export LIBGL_ALWAYS_SOFTWARE=1
  export WGPU_BACKEND="${WGPU_BACKEND:-gl}"
fi

exec nix --extra-experimental-features 'nix-command flakes' develop "$here" --command bash -euc '
  cd "$1"
  root=$1 profile=$2 chrome=$3
  shift 3
  if [ "$chrome" = 1 ]; then
    # Keep the committed lockfile the crates.io one, however this exits.
    saved=$(mktemp)
    cp Cargo.lock "$saved"
    trap "cp \"$saved\" \"$root/Cargo.lock\"; rm -f \"$saved\"" EXIT
    cargo --config third_party/patch.toml build --release -p noctmalia --features wayland-chrome
  elif [ "$profile" = release ]; then
    cargo build --release -p noctmalia
  else
    cargo build -p noctmalia
  fi
  exec "$root/target/$profile/noctmalia" "$@"
' -- "$here" "$profile" "$chrome" "$@"
