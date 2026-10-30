#!/bin/sh
# Builds the web version into a folder served as the site's /app/ (docs/dev/web.md):
#
#   apps/web/build.sh [out-dir]          (default: target/web/app)
#   PROFILE=release apps/web/build.sh    (the default profile is `web`: small, slower to build)
#
# Needs: the target wasm32-unknown-unknown (rustup target add wasm32-unknown-unknown),
# wasm-bindgen-cli of the version in Cargo.lock (cargo install wasm-bindgen-cli --version …;
# WASM_BINDGEN= points at another binary), python3. wasm-opt (binaryen) makes the program
# smaller when it is there.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-$root/target/web/app}
profile=${PROFILE:-web}
bindgen=${WASM_BINDGEN:-wasm-bindgen}

want=$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/version = |"/, ""); print; exit }' "$root/Cargo.lock")
have=$("$bindgen" --version | awk '{ print $2 }')
if [ "$want" != "$have" ]; then
    echo "wasm-bindgen $have, but Cargo.lock has $want: cargo install wasm-bindgen-cli --version $want" >&2
    exit 1
fi

cd "$root"
cargo build -p chesspenguin-web --target wasm32-unknown-unknown --profile "$profile"
wasm="$root/target/wasm32-unknown-unknown/$profile/chesspenguin_web.wasm"

rm -rf "$out"
mkdir -p "$out"
"$bindgen" --target web --no-typescript --out-dir "$out" --out-name chesspenguin_web "$wasm"
if command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -Os --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
        --enable-mutable-globals --enable-reference-types \
        "$out/chesspenguin_web_bg.wasm" -o "$out/chesspenguin_web_bg.wasm"
fi

cp "$here/page/device.js" "$here/page/page.js" "$here/page/import-worker.js" "$out/"

# The engine: Stockfish.js 19.0.1 (npm `stockfish`, GPL-3.0), its single-threaded build,
# checked against the registry's integrity (THIRD_PARTY_NOTICES.md). STOCKFISH_TGZ= points
# at a copy of the package; otherwise it is downloaded once into target/web/.
sf_url=https://registry.npmjs.org/stockfish/-/stockfish-19.0.1.tgz
sf_sha512=bTwsO4e9HAwPBKls1+szcfibmaWRNzTF20rQE/XPdB3LOSoT6XTZDr1zYwriwSRRFT5MkvqrglPmJrKEVXGnyQ==
tgz=${STOCKFISH_TGZ:-$root/target/web/stockfish-19.0.1.tgz}
if [ ! -f "$tgz" ]; then
    mkdir -p "$(dirname "$tgz")"
    curl -fsSL --retry 3 -o "$tgz.part" "$sf_url"
    mv "$tgz.part" "$tgz"
fi
have_sha=$(openssl dgst -sha512 -binary "$tgz" | base64 -w0)
if [ "$have_sha" != "$sf_sha512" ]; then
    echo "$tgz is not stockfish@19.0.1 (sha512 $have_sha)" >&2
    exit 1
fi
mkdir -p "$out/engine"
tar -xzf "$tgz" -C "$out/engine" --strip-components=2 \
    package/bin/stockfish-19-single.js package/bin/stockfish-19-single.wasm
tar -xzf "$tgz" -C "$out/engine" --strip-components=1 package/Copying.txt
for size in 32 64 180; do
    cp "$root/website/static/img/icon-$size.png" "$out/"
done
# The page's texts in the site's languages: the app_* keys and the shared ones it uses.
python3 -I - "$here/page/index.html" "$root/website/site/i18n" "$out/index.html" <<'PY'
import json, pathlib, sys
template, i18n, target = sys.argv[1], pathlib.Path(sys.argv[2]), sys.argv[3]
shared = ["btn_android", "and_light_title", "and_full_title", "and_requires", "next_android",
          "copy_link", "link_copied", "email", "language"]
texts = {}
for f in sorted(i18n.glob("*.json")):
    d = json.loads(f.read_text(encoding="utf-8"))
    texts[f.stem] = {k: v for k, v in d.items() if k.startswith("app_") or k in shared}
page = pathlib.Path(template).read_text(encoding="utf-8")
blob = json.dumps(texts, ensure_ascii=False, separators=(",", ":")).replace("</", "<\\/")
pathlib.Path(target).write_text(page.replace("{{TEXTS}}", blob), encoding="utf-8")
PY
echo "built $out"
du -sh "$out"/chesspenguin_web_bg.wasm
