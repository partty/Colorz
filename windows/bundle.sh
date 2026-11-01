#!/usr/bin/env bash
# Stages a self-contained Colorz tree for the Windows installer: the exe,
# the GTK runtime DLLs it loads, gdbus.exe, the gdk-pixbuf loaders, the
# GTK settings schemas, the Adwaita icon theme and the licenses of every
# bundled MSYS2 package. windows/colorz.iss packs the result.
#
# Usage: windows/bundle.sh <colorz.exe> <stage-dir>
# Runs in the MSYS2 UCRT64 shell. <stage-dir> must not exist or be empty.
set -euo pipefail

die() {
    echo "bundle.sh: error: $*" >&2
    exit 1
}

[ $# -eq 2 ] || die "usage: $0 <colorz.exe> <stage-dir>"
exe=$1
stage=$2
repo=$(cd "$(dirname "$0")/.." && pwd)

prefix=${MSYSTEM_PREFIX:-}
[ -n "$prefix" ] || die "MSYSTEM_PREFIX is not set; run this in the MSYS2 UCRT64 shell"
[ -d "$prefix/bin" ] || die "$prefix/bin does not exist"
[ -f "$exe" ] || die "$exe does not exist"
for tool in ldd pacman glib-compile-schemas gdk-pixbuf-query-loaders gtk4-update-icon-cache; do
    command -v "$tool" >/dev/null || die "$tool is not on PATH"
done
if [ -e "$stage" ] && [ -n "$(ls -A "$stage")" ]; then
    die "$stage is not empty"
fi

loaders_rel=lib/gdk-pixbuf-2.0/2.10.0/loaders
schemas_rel=share/glib-2.0/schemas
mkdir -p "$stage/bin" "$stage/$loaders_rel" "$stage/$schemas_rel" \
    "$stage/share/icons/hicolor" "$stage/share/licenses/colorz"

# Every file copied from $prefix, for finding the packages (and licenses)
# they come from.
owned=$(mktemp)
trap 'rm -f "$owned"' EXIT

# copy <source under $prefix> <destination dir>
copy() {
    [ -f "$1" ] || die "$1 does not exist"
    cp "$1" "$2/"
    printf '%s\n' "$1" >>"$owned"
}

echo "==> Copying colorz.exe and gdbus.exe"
cp "$exe" "$stage/bin/colorz.exe"
copy "$prefix/bin/gdbus.exe" "$stage/bin"

echo "==> Copying gdk-pixbuf loaders"
loaders=("$prefix/$loaders_rel"/*.dll)
[ -f "${loaders[0]}" ] || die "no loader DLLs in $prefix/$loaders_rel"
for f in "${loaders[@]}"; do
    copy "$f" "$stage/$loaders_rel"
    echo "    $(basename "$f")"
done

# The DLLs that `ldd <file>` resolves under $prefix/bin. Missing ones fail.
# The originals are scanned, so the search never finds the staged copies.
prefix_deps() {
    local out
    out=$(ldd "$1") || die "ldd failed on $1"
    if grep -q 'not found' <<<"$out"; then
        die "ldd reports missing DLLs for $1:"$'\n'"$(grep 'not found' <<<"$out")"
    fi
    awk -v p="$prefix/bin/" '$2 == "=>" && index($3, p) == 1 { print $3 }' <<<"$out"
}

echo "==> Collecting DLLs"
declare -A have=()
scan=("$exe" "$prefix/bin/gdbus.exe" "${loaders[@]}")
round=0
while [ ${#scan[@]} -gt 0 ]; do
    round=$((round + 1))
    next=()
    for f in "${scan[@]}"; do
        # Not a process substitution, so a failure stops the script.
        deps=$(prefix_deps "$f")
        while IFS= read -r dll; do
            [ -n "$dll" ] || continue
            name=$(basename "$dll")
            key=${name,,}
            [ -z "${have[$key]:-}" ] || continue
            have[$key]=1
            copy "$dll" "$stage/bin"
            next+=("$dll")
        done <<<"$deps"
    done
    echo "    round $round: ${#next[@]} new"
    scan=("${next[@]+"${next[@]}"}")
done
[ ${#have[@]} -gt 0 ] || die "ldd found no DLLs under $prefix/bin; is $exe a UCRT64 build?"
echo "    ${#have[@]} DLLs"

# gdk-pixbuf on Windows resolves a relative module path in loaders.cache
# against the install prefix (the parent of bin/), so the cache stays valid
# wherever the app is installed.
echo "==> Writing loaders.cache"
cache=$stage/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache
gdk-pixbuf-query-loaders "$stage/$loaders_rel"/*.dll >"$cache.tmp" ||
    die "gdk-pixbuf-query-loaders failed"
sed -E -e '/^# LoaderDir = /d' -e 's#^"([^"]*[/\\])?([^"/\\]+\.dll)"$#"lib\\\\gdk-pixbuf-2.0\\\\2.10.0\\\\loaders\\\\\2"#' \
    "$cache.tmp" >"$cache"
rm "$cache.tmp"
registered=$(grep -c '^"lib\\\\gdk-pixbuf-2.0\\\\2.10.0\\\\loaders\\\\[^"]*\.dll"$' "$cache" || true)
[ "$registered" -eq ${#loaders[@]} ] ||
    die "loaders.cache registers $registered of ${#loaders[@]} loaders:"$'\n'"$(cat "$cache")"
if grep -v '^#' "$cache" | grep -Eq '[A-Za-z]:[/\\]'; then
    die "loaders.cache still has absolute paths:"$'\n'"$(cat "$cache")"
fi
echo "    $registered loaders"

echo "==> Compiling GSettings schemas"
schemas=("$prefix/$schemas_rel"/org.gtk.*.gschema.xml)
[ -f "${schemas[0]}" ] || die "no org.gtk.* schemas in $prefix/$schemas_rel"
for f in "${schemas[@]}" "$prefix/$schemas_rel"/org.gtk.*.enums.xml; do
    [ -f "$f" ] || continue
    copy "$f" "$stage/$schemas_rel"
    echo "    $(basename "$f")"
done
glib-compile-schemas "$stage/$schemas_rel" || die "glib-compile-schemas failed"
[ -f "$stage/$schemas_rel/gschemas.compiled" ] || die "gschemas.compiled was not written"

echo "==> Copying icon themes"
[ -d "$prefix/share/icons/Adwaita" ] || die "$prefix/share/icons/Adwaita does not exist"
cp -R "$prefix/share/icons/Adwaita" "$stage/share/icons/"
find "$prefix/share/icons/Adwaita" -type f >>"$owned"
copy "$prefix/share/icons/hicolor/index.theme" "$stage/share/icons/hicolor"
for theme in Adwaita hicolor; do
    gtk4-update-icon-cache -q -t -f "$stage/share/icons/$theme" ||
        die "gtk4-update-icon-cache failed for $theme"
done

echo "==> Collecting licenses"
pkglist=$(sort -u "$owned" | xargs -d '\n' pacman -Qqo | sort -u) ||
    die "pacman cannot tell which package owns every bundled file"
mapfile -t packages <<<"$pkglist"
[ -n "${packages[0]}" ] || die "pacman found no owning packages"
for pkg in "${packages[@]}"; do
    files=$(pacman -Qlq "$pkg") || die "pacman -Ql $pkg failed"
    count=0
    while IFS= read -r f; do
        case $f in "$prefix/share/licenses/"*[!/]) ;; *) continue ;; esac
        rel=${f#"$prefix/share/licenses/"}
        mkdir -p "$stage/share/licenses/$(dirname "$rel")"
        cp "$f" "$stage/share/licenses/$rel"
        count=$((count + 1))
    done <<<"$files"
    echo "    $pkg: $count license files"
done
{
    echo "Colorz for Windows bundles these MSYS2 packages. Their source code is at"
    echo "https://repo.msys2.org/mingw/sources/ and their build recipes at"
    echo "https://github.com/msys2/MINGW-packages."
    echo
    pacman -Q "${packages[@]}"
} >"$stage/share/licenses/SOURCES.txt"
cp "$repo/LICENSE" "$repo/README.md" "$stage/share/licenses/colorz/"

echo "==> Staged $stage"
echo "    ${#packages[@]} packages, $(find "$stage" -type f | wc -l) files, $(du -sh "$stage" | cut -f1)"
