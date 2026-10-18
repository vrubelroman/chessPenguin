#!/usr/bin/env bash
# Installs ChessPenguin for the current user (~/.local), or removes it with --uninstall.
# Stockfish is downloaded into ~/.local/lib/chesspenguin once (skip with --no-stockfish).
set -euo pipefail

app_id="io.github.vrubelroman.chessPenguin"
root="$(cd "$(dirname "$0")/.." && pwd)"
bin_dir="${HOME}/.local/bin"
lib_dir="${HOME}/.local/lib/chesspenguin"
data_dir="${XDG_DATA_HOME:-${HOME}/.local/share}"
apps_dir="${data_dir}/applications"
icons_dir="${data_dir}/icons/hicolor"
sizes=(16 24 32 48 64 128 256 512)

refresh_caches() {
    command -v update-desktop-database >/dev/null && update-desktop-database -q "${apps_dir}" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "${icons_dir}" || true
}

remove_files() {
    # Current names and the pre-rename development build (chessbase-linux).
    for name in "${app_id}" chessbase-linux; do
        rm -f "${apps_dir}/${name}.desktop"
        for s in "${sizes[@]}"; do rm -f "${icons_dir}/${s}x${s}/apps/${name}.png"; done
    done
    rm -f "${bin_dir}/chesspenguin" "${bin_dir}/chessbase-linux"
}

if [[ "${1:-}" == "--uninstall" ]]; then
    remove_files
    rm -rf "${lib_dir}"
    refresh_caches
    echo "ChessPenguin removed (settings in ~/.config/chesspenguin are kept)."
    exit 0
fi

cargo="${CARGO:-$(command -v cargo || echo "${HOME}/.cargo/bin/cargo")}"
"${cargo}" build --release -p chesspenguin --manifest-path "${root}/Cargo.toml"

remove_files
install -Dm755 "${root}/target/release/chesspenguin" "${bin_dir}/chesspenguin"
install -Dm644 "${root}/packaging/${app_id}.desktop" "${apps_dir}/${app_id}.desktop"
for s in "${sizes[@]}"; do
    install -Dm644 "${root}/packaging/icons/hicolor/${s}x${s}/apps/${app_id}.png" \
        "${icons_dir}/${s}x${s}/apps/${app_id}.png"
done
if [[ "${1:-}" != "--no-stockfish" && ! -x "${lib_dir}/stockfish" ]]; then
    "${root}/packaging/stockfish.sh" linux "${lib_dir}"
fi
refresh_caches
echo "Installed: ${bin_dir}/chesspenguin (launcher entry «ChessPenguin»)."
