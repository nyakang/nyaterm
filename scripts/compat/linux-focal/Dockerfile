# Build on the oldest supported libc, never copy libc from a newer distribution.
FROM ubuntu:20.04@sha256:8feb4d8ca5354def3d8fce243717141ce31e2c428701f6682bd2fafe15388214

ARG DEBIAN_FRONTEND=noninteractive
ARG RUST_VERSION
# Optional combined CA bundle for builds behind a TLS-inspecting proxy.
RUN --mount=type=secret,id=proxy_ca,mode=0444 \
    if [ -f /run/secrets/proxy_ca ]; then \
      export CURL_CA_BUNDLE=/run/secrets/proxy_ca; \
      printf 'Acquire::https::CaInfo "/run/secrets/proxy_ca";\n' > /etc/apt/apt.conf.d/99-build-ca; \
    elif [ ! -s /etc/ssl/certs/ca-certificates.crt ]; then \
      # Minimal Ubuntu has no CA bundle. Bootstrap it using Ubuntu's default \
      # signed APT repositories, then use verified HTTPS for all further downloads. \
      apt-get -o Acquire::Retries=3 update && \
      apt-get -o Acquire::Retries=3 install -y --no-install-recommends ca-certificates || exit 1; \
    fi; \
    sed -i 's|http://archive.ubuntu.com|https://archive.ubuntu.com|g; s|http://security.ubuntu.com|https://security.ubuntu.com|g' /etc/apt/sources.list && \
    apt-get -o Acquire::Retries=3 update && apt-get -o Acquire::Retries=3 install -y --no-install-recommends \
      build-essential dpkg-dev clang ca-certificates curl git pkg-config python3 binutils \
      libgtk-3-dev libayatana-appindicator3-dev libdbus-1-dev \
      libfontconfig1-dev libfreetype6-dev libssl-dev libudev-dev libwayland-dev \
      libx11-dev libx11-xcb-dev libxcb-cursor-dev libxcb-icccm4-dev \
      libxcb-image0-dev libxcb-keysyms1-dev libxcb-randr0-dev libxcb-render0-dev \
      libxcb-shape0-dev libxcb-xfixes0-dev libxcb-xinerama0-dev \
      libxkbcommon-dev libxkbcommon-x11-dev libzstd-dev libvulkan1 \
      mesa-vulkan-drivers libgl1-mesa-dri xvfb xauth x11-utils x11-apps dbus-x11 \
      fonts-dejavu-core && \
    rm -f /etc/apt/apt.conf.d/99-build-ca && rm -rf /var/lib/apt/lists/*

ENV CARGO_HOME=/opt/cargo RUSTUP_HOME=/opt/rustup
ENV PATH=/opt/cargo/bin:$PATH
RUN --mount=type=secret,id=proxy_ca,mode=0444 \
    if [ -f /run/secrets/proxy_ca ]; then \
      export CURL_CA_BUNDLE=/run/secrets/proxy_ca SSL_CERT_FILE=/run/secrets/proxy_ca; \
      cp /etc/ssl/certs/ca-certificates.crt /tmp/base-ca.crt && \
      cat /run/secrets/proxy_ca > /etc/ssl/certs/ca-certificates.crt || exit 1; \
    fi; \
    test -n "$RUST_VERSION" && \
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rustup.sh && \
    sh /tmp/rustup.sh -y --no-modify-path --profile minimal --default-toolchain "$RUST_VERSION" && \
    rm /tmp/rustup.sh && chmod -R a+rX /opt/cargo /opt/rustup && \
    if [ -f /tmp/base-ca.crt ]; then mv /tmp/base-ca.crt /etc/ssl/certs/ca-certificates.crt; fi

# D-Bus requires a passwd entry even when Docker accepts a numeric --user.
ARG BUILDER_UID=1000
ARG BUILDER_GID=1000
RUN if ! getent passwd "$BUILDER_UID" >/dev/null; then \
      if ! getent group "$BUILDER_GID" >/dev/null; then groupadd -g "$BUILDER_GID" builder; fi && \
      useradd -m -u "$BUILDER_UID" -g "$BUILDER_GID" builder; \
    fi

WORKDIR /src
CMD ["bash", "scripts/compat/linux-focal/build.sh"]
