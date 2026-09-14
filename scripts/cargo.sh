#!/usr/bin/env bash
# cargo in this repository's nix devshell, which carries the toolchain and the Wayland/GL libraries
# iced links against. Neither cargo nor rustc is installed on the host.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec nix --extra-experimental-features 'nix-command flakes' develop "$here" --command cargo "$@"
