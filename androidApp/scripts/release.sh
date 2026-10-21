#!/usr/bin/env bash
# Builds the two signed release APKs into androidApp/dist:
#   ChessPenguin-<version>-android.apk           Stockfish built in ("full")
#   ChessPenguin-<version>-android-noengine.apk  Stockfish downloaded in the app ("light")
# The key comes from the environment or from ~/.config/chesspenguin/android/.env.
set -euo pipefail
cd "$(dirname "$0")/.."
env_file="${CP_SIGNING_ENV:-$HOME/.config/chesspenguin/android/.env}"
if [[ -z "${CP_KEYSTORE:-}" && -f "${env_file}" ]]; then
    set -a
    # shellcheck disable=SC1090
    source "${env_file}"
    set +a
fi
: "${CP_KEYSTORE:?no release key: set CP_KEYSTORE and the passwords, or CP_SIGNING_ENV}"
./gradlew --console=plain assembleFullRelease assembleLightRelease
version="$(sed -n 's/^version = "\(.*\)"/\1/p' ../Cargo.toml | head -n1)"
mkdir -p dist
cp app/build/outputs/apk/full/release/app-full-release.apk "dist/ChessPenguin-${version}-android.apk"
cp app/build/outputs/apk/light/release/app-light-release.apk "dist/ChessPenguin-${version}-android-noengine.apk"
build_tools="$(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/build-tools/* | sort -V | tail -n1)"
for apk in dist/ChessPenguin-"${version}"-android*.apk; do
    "${build_tools}/apksigner" verify "${apk}"
    "${build_tools}/zipalign" -c -P 16 4 "${apk}"
done
(cd dist && sha256sum ChessPenguin-"${version}"-android*.apk > ChessPenguin-"${version}"-android.sha256)
ls -l dist
