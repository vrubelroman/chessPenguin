#!/usr/bin/env bash
# Fetches the official Stockfish release bundled with ChessPenguin and checks its SHA-256.
#
#   packaging/stockfish.sh linux   dist/stockfish   -> stockfish, Copying.txt, AUTHORS
#   packaging/stockfish.sh windows dist/stockfish   -> stockfish.exe, Copying.txt, AUTHORS
#   packaging/stockfish.sh source  dist             -> stockfish-<tag>-source.tar.gz (GPL source offer)
#
# The "universal" builds pick the code path for the CPU at run time (x86-64 up to AVX-512),
# so one binary serves every machine. When updating, also update the archive URL and
# sha256 in packaging/flatpak/io.github.vrubelroman.chessPenguin.yml.
set -euo pipefail

tag="sf_19"
base="https://github.com/official-stockfish/Stockfish/releases/download/${tag}"
linux_asset="stockfish-linux-x86-64-universal.tar.gz"
linux_sha256="9defc0d4e55d49c65a6d042f3e571a39fcea499ade6dbe741b53b8c65e03611f"
windows_asset="stockfish-windows-x86-64-universal.zip"
windows_sha256="3c8bf1f9ea66a09350a40df4f632288285ac206d99f33ab5842c408fc30b48a7"

target="${1:?usage: $0 linux|windows|source OUT_DIR}"
out="${2:?usage: $0 linux|windows|source OUT_DIR}"
mkdir -p "${out}"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

fetch() { # asset sha256
    curl -fsSL --retry 3 -o "${work}/$1" "${base}/$1"
    echo "$2  ${work}/$1" | sha256sum -c --quiet -
}

# Copies the engine executable matching a glob plus the license files from an unpacked release.
install_from() { # dir glob dest_name
    local exe
    exe="$(find "$1" -type f -name "$2" | head -n1)"
    [[ -n "${exe}" ]] || { echo "stockfish.sh: no $2 in the archive" >&2; exit 1; }
    install -m755 "${exe}" "${out}/$3"
    for f in Copying.txt AUTHORS; do
        local src
        src="$(find "$1" -type f -name "${f}" | head -n1)"
        [[ -n "${src}" ]] && install -m644 "${src}" "${out}/${f}"
    done
}

case "${target}" in
    linux)
        fetch "${linux_asset}" "${linux_sha256}"
        tar -xzf "${work}/${linux_asset}" -C "${work}"
        install_from "${work}" "stockfish-linux-x86-64*" stockfish
        ;;
    windows)
        fetch "${windows_asset}" "${windows_sha256}"
        python3 -m zipfile -e "${work}/${windows_asset}" "${work}/x" 2>/dev/null \
            || python -m zipfile -e "${work}/${windows_asset}" "${work}/x"
        install_from "${work}/x" "stockfish-windows-x86-64*.exe" stockfish.exe
        ;;
    source)
        curl -fsSL --retry 3 -o "${out}/stockfish-${tag}-source.tar.gz" \
            "https://github.com/official-stockfish/Stockfish/archive/refs/tags/${tag}.tar.gz"
        ;;
    *)
        echo "usage: $0 linux|windows|source OUT_DIR" >&2
        exit 2
        ;;
esac
echo "Stockfish ${tag} (${target}) -> ${out}"
