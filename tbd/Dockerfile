# syntax=docker/dockerfile:1
# tbd — Thunderbird as a headless mail/calendar daemon for noctmalia.
# Mozilla publishes Linux release builds for x86_64 only; on arm64 hosts this runs emulated.

FROM --platform=$BUILDPLATFORM debian:trixie-slim AS fetch
ARG TB_VERSION=155.0.1
# From https://archive.mozilla.org/pub/thunderbird/releases/155.0.1/SHA512SUMS
ARG TB_SHA512=76625ec65ea09b723e3b0492ebb660e1e15d8efdcb3dbb5e6ea31fc22d6c7161bde6ddece2286751d0bf4263fc61ed3c76e5874706d15184d8b0c424a93549d9
RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends ca-certificates curl xz-utils zip >/dev/null \
 && rm -rf /var/lib/apt/lists/*
RUN curl -fsSLo /tmp/tb.tar.xz "https://archive.mozilla.org/pub/thunderbird/releases/${TB_VERSION}/linux-x86_64/en-US/thunderbird-${TB_VERSION}.tar.xz" \
 && echo "${TB_SHA512}  /tmp/tb.tar.xz" | sha512sum -c - \
 && tar -xJf /tmp/tb.tar.xz -C /opt \
 && rm /tmp/tb.tar.xz
COPY bridge /src/bridge
RUN mkdir -p /out && cd /src/bridge && zip -qr -X /out/noctmalia-bridge.xpi .

FROM --platform=linux/amd64 debian:trixie-slim
RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      ca-certificates python3 fonts-dejavu-core \
      libgtk-3-0t64 libdbus-glib-1-2 libasound2t64 libx11-xcb1 libxtst6 libpci3 >/dev/null \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m -u 1000 tb \
 && install -d -o tb -g tb /data/profile /run/noctmalia
COPY --from=fetch /opt/thunderbird /opt/thunderbird
COPY --from=fetch /out/noctmalia-bridge.xpi /opt/noctmalia/noctmalia-bridge.xpi
COPY --chmod=0755 shim/nm-shim.py /opt/noctmalia/nm-shim
COPY --chmod=0755 entrypoint.sh /opt/noctmalia/entrypoint.sh
COPY prefs/user.js /opt/noctmalia/user.js

USER tb
ENV TBD_MODE=headless \
    NOCTMALIA_BRIDGE_SOCKET=/run/noctmalia/bridge.sock
VOLUME /data/profile
# Healthy once the bridge extension is running and has spawned the shim.
HEALTHCHECK --interval=15s --start-period=180s \
  CMD sh -c 'read pid state < /tmp/nm-shim.state && kill -0 "$pid"'
ENTRYPOINT ["/opt/noctmalia/entrypoint.sh"]
