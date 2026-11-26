#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <directory containing digests-macos, digests-linux and digests-windows>" >&2
    exit 2
fi

root=$1
for platform in macos linux windows; do
    set -- "$root/digests-$platform/"*.json
    if [ ! -f "$1" ]; then
        echo "Missing determinism digests for $platform" >&2
        exit 1
    fi
done

status=0
for platform in linux windows; do
    diff -ru "$root/digests-macos" "$root/digests-$platform" || status=1
done

if [ "$status" -eq 0 ]; then
    echo 'Determinism digests match on macOS, Linux and Windows.'
fi
exit "$status"
