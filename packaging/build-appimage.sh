#!/usr/bin/env bash
# Builds ChessPenguin-<version>-x86_64.AppImage from target/release/chesspenguin (the
# program downloads Stockfish on request).
# Usage (repository root): VERSION=0.1.0 packaging/build-appimage.sh dist/
set -euo pipefail

app_id="io.github.vrubelroman.chessPenguin"
out_dir="${1:-dist}"
version="${VERSION:?set VERSION}"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

curl -fsSL -o "${work}/linuxdeploy" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
curl -fsSL -o "${work}/appimagetool" \
    https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
chmod +x "${work}/linuxdeploy" "${work}/appimagetool"

appdir="${work}/AppDir"
install -Dm755 target/release/chesspenguin "${appdir}/usr/bin/chesspenguin"
# AppStream metadata is left out: appimagetool validates its screenshot URLs over the network.
for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 "packaging/icons/hicolor/${size}x${size}/apps/${app_id}.png" \
        "${appdir}/usr/share/icons/hicolor/${size}x${size}/apps/${app_id}.png"
done

# 1. linuxdeploy fills the AppDir (dependencies, AppRun, desktop entry, icon).
APPIMAGE_EXTRACT_AND_RUN=1 "${work}/linuxdeploy" \
    --appdir "${appdir}" \
    --desktop-file "packaging/${app_id}.desktop" \
    --icon-file "packaging/icons/hicolor/256x256/apps/${app_id}.png"

# 2. appimagetool packs the AppDir as is.
mkdir -p "${out_dir}"
output="${out_dir}/ChessPenguin-${version}-x86_64.AppImage"
ARCH=x86_64 VERSION="${version}" APPIMAGE_EXTRACT_AND_RUN=1 \
    "${work}/appimagetool" --no-appstream "${appdir}" "${output}"
echo "Built ${output}"
