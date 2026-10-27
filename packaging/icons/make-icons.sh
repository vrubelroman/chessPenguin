#!/bin/sh
# Makes every icon of ChessPenguin from one picture (default icons/icon2.png): the Linux
# hicolor PNGs (also the macOS .icns and the packages take them), the window icon, the
# Windows .ico, the Android launcher icon (adaptive foreground and monochrome), the
# app's own picture and the site's icons. Needs ImageMagick 7 (`magick`). Run from the repository's root:
#   packaging/icons/make-icons.sh [picture.png]
# The site's link preview (website/static/og-image.png) is made by website/shots/derive.py
# from the site's icon.
set -eu
src="${1:-icons/icon2.png}"
app_id=io.github.vrubelroman.chessPenguin
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# The picture without its empty border, centred on a square with a small margin.
magick "$src" -trim +repage -resize 940x940 -background none -gravity center \
    -extent 1024x1024 "$tmp/master.png"

for s in 16 24 32 48 64 128 256 512; do
    magick "$tmp/master.png" -filter Lanczos -resize "${s}x${s}" \
        "packaging/icons/hicolor/${s}x${s}/apps/${app_id}.png"
done
cp "packaging/icons/hicolor/256x256/apps/${app_id}.png" apps/desktop/assets/icon-256.png
magick "$tmp/master.png" -define icon:auto-resize=256,128,64,48,32,16 \
    packaging/windows/chesspenguin.ico

# Android: the foreground of the adaptive icon is 108 dp, of which a circle of 66 dp is
# always seen: the picture fills 62 % of it. The monochrome one (themed icons) is one
# solid colour: everything but the white of the belly and face, which is cut out, so the
# face stays readable.
res=androidApp/app/src/main/res
magick "$tmp/master.png" -resize 635x635 -background none -gravity center \
    -extent 1024x1024 "$tmp/fg.png"
magick "$tmp/fg.png" -alpha extract -threshold 50% "$tmp/shape.png"
magick "$tmp/fg.png" -alpha off -colorspace Gray -threshold 78% -negate "$tmp/dark.png"
magick "$tmp/shape.png" "$tmp/dark.png" -compose Multiply -composite "$tmp/mono-alpha.png"
magick -size 1024x1024 xc:white "$tmp/mono-alpha.png" -alpha off \
    -compose CopyOpacity -composite "$tmp/mono.png"
for d in mdpi:108 hdpi:162 xhdpi:216 xxhdpi:324 xxxhdpi:432; do
    dir="${d%%:*}"; px="${d#*:}"
    magick "$tmp/fg.png" -filter Lanczos -resize "${px}x${px}" -quality 95 \
        "$res/mipmap-$dir/ic_launcher_foreground.webp"
    magick "$tmp/mono.png" -filter Lanczos -resize "${px}x${px}" -quality 95 \
        "$res/mipmap-$dir/ic_launcher_monochrome.webp"
done

# Android, inside the app: the picture of «About» and of the empty list of databases.
magick "$tmp/master.png" -filter Lanczos -resize 288x288 "$res/drawable-nodpi/penguin.png"

# The site: tab icons, the header logo, the home screen icon of iOS (opaque, as iOS
# fills a transparent one with black).
img=website/static/img
for s in 16 32 64; do
    magick "$tmp/master.png" -filter Lanczos -resize "${s}x${s}" "$img/icon-$s.png"
done
magick "$tmp/master.png" -resize 150x150 -background '#EAF2F9' -gravity center \
    -extent 180x180 -alpha remove "$img/icon-180.png"
echo "icons made from $src"
