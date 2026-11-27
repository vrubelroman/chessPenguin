#!/usr/bin/env bash
# Run a GUI command on an invisible Hyprland monitor so test windows never appear on the
# owner's screens and never take focus or the cursor.
#
#   scripts/dev/headless-run.sh [--timeout SEC] -- ENV=VAL... command args...
#   scripts/dev/headless-run.sh --remove        # remove the virtual monitor
#
# The command runs in the current directory with the given environment (CP_SCREENSHOT,
# CP_REPERTOIRE_DIR, ...); stdout/stderr are printed when it exits; the exit code is kept.
# The virtual monitor is 1920x1080, scale 1, far from the real screens (the cursor cannot
# reach it). Windows are opened with the «silent» workspace rule: no focus change.
# Do not launch test windows any other way (plain launch puts them under the owner's mouse;
# the no_initial_focus rule does not work here and warps the cursor).
set -euo pipefail

OUTPUT=CPTEST
timeout=300

die() { echo "headless-run: $*" >&2; exit 2; }
command -v hyprctl >/dev/null || die "hyprctl not found (not Hyprland?)"
[[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || die "not inside a Hyprland session"

monitor_ws() {
    hyprctl monitors -j | python3 -c '
import json, sys
for m in json.load(sys.stdin):
    if m["name"] == sys.argv[1]:
        print(m["activeWorkspace"]["id"])
' "$OUTPUT"
}

if [[ "${1:-}" == "--remove" ]]; then
    hyprctl output remove "$OUTPUT" >/dev/null || true
    exit 0
fi
if [[ "${1:-}" == "--timeout" ]]; then timeout="$2"; shift 2; fi
[[ "${1:-}" == "--" ]] && shift
(( $# > 0 )) || die "no command"

ws="$(monitor_ws)"
if [[ -z "$ws" ]]; then
    hyprctl output create headless "$OUTPUT" >/dev/null
    sleep 0.5
    hyprctl eval "hl.monitor({ output = \"$OUTPUT\", mode = \"1920x1080@60\", position = \"10000x10000\", scale = 1 })" >/dev/null
    sleep 0.5
    ws="$(monitor_ws)"
    [[ -n "$ws" ]] || die "virtual monitor $OUTPUT was not created"
fi

run_dir="$(mktemp -d /tmp/opencode/headless-run.XXXXXX)"
inner="$run_dir/inner.sh"
{
    echo '#!/usr/bin/env bash'
    printf 'cd %q\n' "$PWD"
    # Pass through the session variables the app needs, then the caller's ones.
    for v in WAYLAND_DISPLAY XDG_RUNTIME_DIR HOME PATH LANG CARGO_HOME RUSTUP_HOME; do
        [[ -n "${!v:-}" ]] && printf 'export %s=%q\n' "$v" "${!v}"
    done
    printf 'exec env'
    printf ' %q' "$@"
    printf ' >%q 2>&1\n' "$run_dir/log"
} >"$inner"
chmod +x "$inner"
wrapper="$run_dir/wrapper.sh"
printf '#!/usr/bin/env bash\n%q; echo $? >%q\n' "$inner" "$run_dir/status" >"$wrapper"
chmod +x "$wrapper"

# Floating at the monitor's origin with the full size: parallel runs overlap instead of
# tiling, so every window is exactly 1920x1080.
hyprctl eval "hl.exec_cmd(\"$wrapper\", { workspace = \"$ws silent\", float = true, size = \"1920 1080\", move = \"0 0\" })" >/dev/null

waited=0
while [[ ! -s "$run_dir/status" ]]; do
    sleep 1
    waited=$((waited + 1))
    if (( waited >= timeout )); then
        pkill -f -- "$inner" || true
        cat "$run_dir/log" 2>/dev/null || true
        die "timeout after ${timeout}s (logs: $run_dir)"
    fi
done
cat "$run_dir/log"
exit "$(cat "$run_dir/status")"
