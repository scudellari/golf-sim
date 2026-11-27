#!/bin/sh
set -eu

status=0
output=$(just debug-replay testdata/bundles/lip-out --batch --no-lldbinit \
    -o 'breakpoint set --file bounce.rs --line 193' \
    -o run -o 'thread backtrace' 2>&1) || status=$?
printf '%s\n' "$output"
[ "$status" -eq 0 ] || exit "$status"

if ! printf '%s\n' "$output" | grep -q 'stop reason = breakpoint' ||
   ! printf '%s\n' "$output" | grep -Eq 'frame #[0-9]+: .*resolve_impact.* at .*bounce\.rs:[0-9]+' ||
   ! printf '%s\n' "$output" | grep -Eq 'frame #[1-9][0-9]*: .* at .*integrator\.rs:[0-9]+'; then
    echo 'Expected a breakpoint in bounce::resolve_impact and source lines for it and its caller.' >&2
    exit 1
fi
