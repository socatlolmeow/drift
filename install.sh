#!/bin/sh
set -eu

have() { command -v "$1" >/dev/null 2>&1; }

if [ "$(id -u)" -eq 0 ]; then
    SU=
elif have sudo; then
    SU=sudo
elif have doas; then
    SU=doas
else
    echo "Error: need root, sudo or doas." >&2
    exit 1
fi

install_packages() { $SU "$@"; }

echo "-> Detecting package manager..."

if have pacman; then
    install_packages pacman -S --needed --noconfirm \
        mpv yt-dlp base-devel curl

elif have apt-get; then
    install_packages apt-get update
    install_packages apt-get install -y \
        mpv yt-dlp build-essential curl ca-certificates

elif have dnf; then
    install_packages dnf install -y \
        mpv yt-dlp gcc gcc-c++ make curl

elif have zypper; then
    install_packages zypper --non-interactive install \
        mpv yt-dlp gcc gcc-c++ make curl

elif have xbps-install; then
    install_packages xbps-install -Sy \
        mpv yt-dlp base-devel curl

elif have apk; then
    install_packages apk add \
        mpv yt-dlp build-base curl

elif have emerge; then
    install_packages emerge --noreplace \
        media-video/mpv net-misc/yt-dlp \
        sys-devel/gcc net-misc/curl

else
    echo "Error: unsupported package manager." >&2
    echo "Install mpv, yt-dlp, a C compiler, curl and rustup manually." >&2
    exit 1
fi

echo "-> Setting up Rust toolchain..."

[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

if have rustup; then
    rustup default stable
elif have cargo; then
    echo "-> Using existing cargo: $(cargo --version)"
else
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --profile minimal --default-toolchain stable
    . "$HOME/.cargo/env"
fi

echo "-> Checking required commands..."

for cmd in cargo mpv yt-dlp; do
    if ! have "$cmd"; then
        echo "Error: $cmd was not found after installation." >&2
        exit 1
    fi
done

echo "-> Installing drift..."

cargo install --git https://github.com/socatlolmeow/drift

echo "-> Drift installation complete!"
echo "Run 'drift' to start the application."
echo "If the command is not found, pls check that ~/.cargo/bin is in your PATH"
