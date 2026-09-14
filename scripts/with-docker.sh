#!/usr/bin/env bash
# Run a command with access to the docker socket.
#
#   scripts/with-docker.sh docker compose ps
#   scripts/with-docker.sh tools/seed.sh --reset
#
# `docker.socket` is socket-activated on Arch, so `sudo usermod -aG docker "$USER"` is usually the
# only thing missing. But a new group is not live in a shell that was already open, and `groups`
# reports it before the kernel has given the process those credentials — so it says yes while docker
# says permission denied. The only honest test is whether the socket answers; when it does not, this
# re-runs itself under the group rather than asking anyone to log out.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: ${0##*/} <command> [args...]" >&2
  exit 2
fi

if docker info >/dev/null 2>&1; then
  exec "$@"
fi

# Second time through and still nothing: the user is not in the group at all, which no amount of
# re-execing fixes.
if [ "${NOCTMALIA_IN_DOCKER_GROUP:-}" = 1 ]; then
  echo "${0##*/}: the docker socket is unreachable even under the docker group." >&2
  echo "  add yourself to it:  sudo usermod -aG docker \"\$USER\"" >&2
  echo "  and check the daemon is up:  systemctl status docker.socket" >&2
  exit 1
fi

# newgrp takes its commands on stdin. printf %q keeps arguments intact through the extra shell.
exec newgrp docker <<EOF
NOCTMALIA_IN_DOCKER_GROUP=1 exec $(printf '%q ' "$0" "$@")
EOF
