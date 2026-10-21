#!/usr/bin/env bash
# Builds Stockfish for Android from the pinned official source, without the network
# inside (the app passes it with EvalFile), and fetches that network.
#
#   scripts/stockfish.sh OUT_DIR
#     OUT_DIR/jniLibs/arm64-v8a/libstockfish.so, OUT_DIR/jniLibs/x86_64/libstockfish.so
#     OUT_DIR/net/<network>.nnue, OUT_DIR/source/Copying.txt, AUTHORS
#
# The executable goes into the APK as a "library", the only place Android 10+ lets an
# app execute a file from (docs/research/android-stack.md §5). Downloads are cached in
# ~/.cache/chesspenguin-android and checked against their SHA-256.
# Needs ANDROID_NDK_HOME (NDK r27c or later).
set -euo pipefail

tag="sf_19"
source_sha256="519b653d0d1ffb96531d982ccbe5c6a19425e8388e0e3c2f70f34b424ab32d76"
net="nn-1a298aa575a0.nnue"
net_sha256="1a298aa575a085434d29027978dc36867fe9c5bcea9376654b7a8eba1e52dfc2"

out="$(realpath -m "${1:?usage: $0 OUT_DIR}")"
ndk="${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME}"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/chesspenguin-android"
mkdir -p "${cache}" "${out}"

fetch() { # file sha256 url...
    local file="$1" sha="$2"
    shift 2
    if [[ -f "${cache}/${file}" ]] && echo "${sha}  ${cache}/${file}" | sha256sum -c --quiet - 2>/dev/null; then
        return
    fi
    for url in "$@"; do
        if curl -fsSL --retry 3 --max-time 900 -o "${cache}/${file}.part" "${url}" \
            && echo "${sha}  ${cache}/${file}.part" | sha256sum -c --quiet -; then
            mv "${cache}/${file}.part" "${cache}/${file}"
            return
        fi
        echo "stockfish.sh: ${url} failed or did not match" >&2
    done
    rm -f "${cache}/${file}.part"
    exit 1
}

fetch "${tag}.tar.gz" "${source_sha256}" \
    "https://github.com/official-stockfish/Stockfish/archive/refs/tags/${tag}.tar.gz"
fetch "${net}" "${net_sha256}" \
    "https://github.com/official-stockfish/networks/raw/master/${net}" \
    "https://tests.stockfishchess.org/api/nn/${net}"

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT
tar -xzf "${cache}/${tag}.tar.gz" -C "${work}"
src="$(find "${work}" -maxdepth 2 -type d -name src | head -n1)"
# The Makefile checks the network before building; it finds it in place.
cp "${cache}/${net}" "${src}/${net}"

export PATH="${ndk}/toolchains/llvm/prebuilt/linux-x86_64/bin:${PATH}"
build() { # abi arch
    make -C "${src}" -s clean >/dev/null
    make -C "${src}" -j"$(nproc)" build ARCH="$2" COMP=ndk EXTRACXXFLAGS=-DNNUE_EMBEDDING_OFF >/dev/null
    mkdir -p "${out}/jniLibs/$1"
    llvm-strip -o "${out}/jniLibs/$1/libstockfish.so" "${src}/stockfish"
}
build arm64-v8a armv8
build x86_64 x86-64-sse41-popcnt

mkdir -p "${out}/net" "${out}/source"
cp "${cache}/${net}" "${out}/net/${net}"
cp "${src}/../Copying.txt" "${src}/../AUTHORS" "${out}/source/"
echo "Stockfish ${tag} for Android -> ${out}"
