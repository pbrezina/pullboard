#!/bin/sh
# Package an already built/tested artifact. Never rebuild or modify the binary.
set -eu
if [ "$#" -ne 2 ] || [ ! -f "$1" ] || [ ! -x "$1" ]; then
    printf '%s\n' 'Usage: package-setup.sh TESTED_EXECUTABLE OUTPUT_DIRECTORY' >&2
    exit 1
fi
binary=$1
output=$2
script_directory=$(CDPATH= cd -P "$(dirname "$0")" && pwd)
mkdir -p "$output"
asset=pullboard-setup-linux-x86_64.gz
gzip -9n -c "$binary" > "$output/$asset"
cp "$script_directory/pullboard-setup.sh" "$output/pullboard-setup.sh"
chmod 755 "$output/pullboard-setup.sh"
raw_bytes=$(wc -c < "$binary" | tr -d '[:space:]')
compressed_bytes=$(wc -c < "$output/$asset" | tr -d '[:space:]')
printf 'Setup executable: %s bytes raw; %s bytes gzip.\n' "$raw_bytes" "$compressed_bytes" > "$output/setup-sizes.txt"
(cd "$output" && sha256sum "$asset" pullboard-setup.sh setup-sizes.txt > SHA256SUMS)
cat "$output/setup-sizes.txt"
