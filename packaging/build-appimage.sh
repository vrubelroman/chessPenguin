#!/usr/bin/env bash
# Builds ChessPenguin-<version>-x86_64.AppImage from target/release/chesspenguin.
# Usage (repository root): VERSION=0.1.0 packaging/build-appimage.sh dist/
set -euo pipefail

app_id="io.github.vrubelroman.chessPenguin"
out_dir="${1:-dist}"
version="${VERSION:?set VERSION}"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

curl -fsSL -o "${work}/linuxdeploy" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
chmod +x "${work}/linuxdeploy"

appdir="${work}/AppDir"
install -Dm755 target/release/chesspenguin "${appdir}/usr/bin/chesspenguin"
# AppStream metadata is left out: appimagetool validates its screenshot URLs over the network.
for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 "packaging/icons/hicolor/${size}x${size}/apps/${app_id}.png" \
        "${appdir}/usr/share/icons/hicolor/${size}x${size}/apps/${app_id}.png"
done

mkdir -p "${out_dir}"
export LINUXDEPLOY_OUTPUT_VERSION="${version}"
export OUTPUT="${out_dir}/ChessPenguin-${version}-x86_64.AppImage"
APPIMAGE_EXTRACT_AND_RUN=1 "${work}/linuxdeploy" \
    --appdir "${appdir}" \
    --desktop-file "packaging/${app_id}.desktop" \
    --icon-file "packaging/icons/hicolor/256x256/apps/${app_id}.png" \
    --output appimage
echo "Built ${OUTPUT}"
