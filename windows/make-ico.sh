#!/usr/bin/env bash
# Regenerates windows/colorz.ico from the app icon SVG. Run from the repo
# root after changing data/io.github.partty.Colorz.svg, and commit the result.
# Needs ImageMagick 7 (`magick`) and icoutils (`icotool`).
set -euo pipefail

svg=data/io.github.partty.Colorz.svg
out=windows/colorz.ico
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

args=()
for n in 16 20 24 32 40 48 64 256; do
    magick -background none -density 384 "$svg" -resize "${n}x${n}" -depth 8 "png:-" >"$tmp/$n.png"
    if [ "$n" = 256 ]; then
        # The 256 px image stays PNG-compressed, as Windows expects.
        args+=(--raw="$tmp/$n.png")
    else
        args+=("$tmp/$n.png")
    fi
done

icotool -c -o "$out" "${args[@]}"
echo "wrote $out"
