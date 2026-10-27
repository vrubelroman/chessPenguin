#!/usr/bin/env bash
# Builds Rodent IV (github.com/nescitus/rodent-iv, GPL-3.0) for Android from the authors'
# sources at the commit the desktop build uses (.github/workflows/rodent-iv.yml), with the
# same fix (packaging/rodent-iv/home-dir.patch), and two more: think-thread.patch (a
# dangling reference that clang builds trip over) and android.patch (bionic has no
# wordexp); takes its personalities.
#
#   scripts/rodent4.sh OUT_DIR
#     OUT_DIR/jniLibs/arm64-v8a/librodent4.so, OUT_DIR/jniLibs/x86_64/librodent4.so
#     OUT_DIR/home/personalities/*  (basic.ini and the personality files)
#     OUT_DIR/source/LICENSE, README.md, COPYRIGHT, books/<folder>/readme.txt (the books'
#       credits, the authors' #readme.txt)
#
# The executable goes into the APK as a "library", the only place Android 10+ lets an
# app execute a file from (docs/research/android-stack.md §5). The engine finds its
# personalities and books in the folder named by RODENT4HOME: NDK clang defines
# __ANDROID__ but not ANDROID, so the authors' /sdcard/Rodent4 branch is not compiled
# (android.patch keys only the missing wordexp on __ANDROID__).
# The books are not here: the app downloads them. libc++ is linked statically.
# The source is cached in ~/.cache/chesspenguin-android and checked against its SHA-256.
# Needs ANDROID_NDK_HOME (NDK r27c or later).
set -euo pipefail

commit="e8d84c8c8c189a1cf4eb27c578fc573af3b916d2"
source_sha256="db372b48a8ccf22d9bfbe3e5454cbbccb81dadca0120cbc0abaee2a81a0cb3fe"
api=31

here="$(cd "$(dirname "$0")" && pwd)"
patches="${here}/../../packaging/rodent-iv"
out="$(realpath -m "${1:?usage: $0 OUT_DIR}")"
ndk="${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME}"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/chesspenguin-android"
mkdir -p "${cache}" "${out}"

file="rodent-iv-${commit}.tar.gz"
if ! { [[ -f "${cache}/${file}" ]] \
    && echo "${source_sha256}  ${cache}/${file}" | sha256sum -c --quiet - 2>/dev/null; }; then
    curl -fsSL --retry 3 --max-time 900 -o "${cache}/${file}.part" \
        "https://github.com/nescitus/rodent-iv/archive/${commit}.tar.gz"
    if ! echo "${source_sha256}  ${cache}/${file}.part" | sha256sum -c --quiet -; then
        rm -f "${cache}/${file}.part"
        echo "rodent4.sh: the source does not match its SHA-256" >&2
        exit 1
    fi
    mv "${cache}/${file}.part" "${cache}/${file}"
fi

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT
tar -xzf "${cache}/${file}" -C "${work}"
root="${work}/rodent-iv-${commit}"
patch -s -d "${root}" -p1 < "${patches}/home-dir.patch"
patch -s -d "${root}" -p1 < "${patches}/think-thread.patch"
patch -s -d "${root}" -p1 < "${patches}/android.patch"

bin="${ndk}/toolchains/llvm/prebuilt/linux-x86_64/bin"
build() { # abi target
    "${bin}/$2${api}-clang++" -O3 -w -pipe -DNDEBUG -fno-rtti -finline-functions -std=c++14 \
        -static-libstdc++ "${root}"/sources/src/*.cpp -o "${work}/rodent4-$1" -lm
    mkdir -p "${out}/jniLibs/$1"
    "${bin}/llvm-strip" -o "${out}/jniLibs/$1/librodent4.so" "${work}/rodent4-$1"
}
build arm64-v8a aarch64-linux-android
build x86_64 x86_64-linux-android

rm -rf "${out}/home" "${out}/source"
mkdir -p "${out}/home" "${out}/source"
cp -r "${root}/personalities" "${out}/home/personalities"
cp "${root}/LICENSE" "${root}/README.md" "${out}/source/"
# The authors are named in the sources' headers; the repository has no AUTHORS file.
sed -n '/^\/\*/,/^\*\//p' "${root}/sources/src/main.cpp" > "${out}/source/COPYRIGHT"
# The books are downloaded by the app; their credits come with the licence.
for credits in $(cd "${root}" && find books -name '#readme.txt'); do
    mkdir -p "${out}/source/$(dirname "${credits}")"
    cp "${root}/${credits}" "${out}/source/$(dirname "${credits}")/readme.txt"
done
echo "Rodent IV (${commit:0:8}) for Android -> ${out}"
