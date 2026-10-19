#!/usr/bin/env bash
# Builds ChessPenguin.app and ChessPenguin-<version>-macos-arm64.dmg from
# target/release/chesspenguin on macOS (decision 15). The app is signed ad hoc only:
# Apple Silicon refuses unsigned code, and a Developer ID signature with notarization
# needs an Apple developer account. The first start goes through "Open" in the context
# menu (or System Settings -> Privacy & Security -> Open Anyway).
# Usage (repository root): VERSION=0.6.0 packaging/macos/build-app.sh dist/
set -euo pipefail

out_dir="${1:?usage: $0 OUT_DIR}"
version="${VERSION:?set VERSION}"
app_id="io.github.vrubelroman.chessPenguin"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

app="${work}/ChessPenguin.app"
mkdir -p "${app}/Contents/MacOS" "${app}/Contents/Resources"
install -m755 target/release/chesspenguin "${app}/Contents/MacOS/ChessPenguin"
cp LICENSE THIRD_PARTY_NOTICES.md "${app}/Contents/Resources/"
cp -r examples "${app}/Contents/Resources/"

# Icon: an .iconset from the PNGs (iconutil wants these exact names).
iconset="${work}/ChessPenguin.iconset"
mkdir -p "${iconset}"
icon() { cp "packaging/icons/hicolor/$1x$1/apps/${app_id}.png" "${iconset}/$2"; }
icon 16 icon_16x16.png
icon 32 icon_16x16@2x.png
icon 32 icon_32x32.png
icon 64 icon_32x32@2x.png
icon 128 icon_128x128.png
icon 256 icon_128x128@2x.png
icon 256 icon_256x256.png
icon 512 icon_256x256@2x.png
icon 512 icon_512x512.png
iconutil -c icns -o "${app}/Contents/Resources/ChessPenguin.icns" "${iconset}"

cat > "${app}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>ChessPenguin</string>
  <key>CFBundleDisplayName</key><string>ChessPenguin</string>
  <key>CFBundleIdentifier</key><string>${app_id}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleExecutable</key><string>ChessPenguin</string>
  <key>CFBundleIconFile</key><string>ChessPenguin</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.board-games</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Chess games</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>CFBundleTypeExtensions</key>
      <array><string>pgn</string><string>cbh</string><string>cbv</string></array>
    </dict>
  </array>
</dict>
</plist>
EOF

codesign --force --deep --sign - "${app}"

# Disk image with the app and a link to /Applications.
staging="${work}/dmg"
mkdir -p "${staging}"
cp -R "${app}" "${staging}/"
ln -s /Applications "${staging}/Applications"
mkdir -p "${out_dir}"
output="${out_dir}/ChessPenguin-${version}-macos-arm64.dmg"
hdiutil create -volname "ChessPenguin ${version}" -srcfolder "${staging}" -ov -format UDZO "${output}"
echo "Built ${output}"
