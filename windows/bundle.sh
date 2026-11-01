#!/usr/bin/env bash
# Stages a self-contained Colorz tree for the Windows installer: the exe,
# the GTK runtime DLLs it loads, the gdk-pixbuf loaders, the GTK settings
# schemas, the Adwaita icon theme and the licenses of every bundled MSYS2
# package. windows/colorz.iss packs the result. There is no gdbus.exe:
# Colorz keeps off the D-Bus session bus on Windows.
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

echo "==> Copying colorz.exe"
cp "$exe" "$stage/bin/colorz.exe"

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
scan=("$exe" "${loaders[@]}")
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
# A native Windows program, so its output may end lines with CRLF.
sed -i 's/\r$//' "$cache.tmp"
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
# One owned file stands for the theme: the theme ships generated, unowned
# files (icon-theme.cache), and a lookup per icon would be slow.
[ -f "$prefix/share/icons/Adwaita/index.theme" ] || die "Adwaita has no index.theme"
printf '%s\n' "$prefix/share/icons/Adwaita/index.theme" >>"$owned"
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

# Packages that install no license file of their own, mapped to an
# installed copy of their license's text and the pacman License value it
# fits. Every other package must ship at least one license file. All of
# them are LGPL-2.1 today, which the text check below relies on.
lgpl21=$prefix/share/licenses/libadwaita/COPYING
declare -A fallback_text=(
    [gstreamer]=$lgpl21
    [gst-plugins-base]=$lgpl21
    [gst-plugins-bad-libs]=$lgpl21
)
declare -A fallback_license=(
    [gstreamer]=LGPL-2.1-or-later
    [gst-plugins-base]=LGPL-2.1-or-later
    [gst-plugins-bad-libs]=LGPL-2.1-or-later
)

# The License field of an installed package, e.g.
# "spdx:LGPL-2.1-or-later" (MSYS2 marks SPDX identifiers with "spdx:").
pkg_license() {
    local out
    out=$(LC_ALL=C pacman -Qi "$1") || die "pacman -Qi $1 failed"
    sed -n 's/^Licenses *: *//p' <<<"$out"
}

sources=()
for pkg in "${packages[@]}"; do
    license=$(pkg_license "$pkg")
    [ -n "$license" ] || die "$pkg has no License field"
    sources+=("$(LC_ALL=C pacman -Q "$pkg")  License: $license")
    files=$(pacman -Qlq "$pkg") || die "pacman -Ql $pkg failed"
    count=0
    while IFS= read -r f; do
        case $f in "$prefix/share/licenses/"*[!/]) ;; *) continue ;; esac
        rel=${f#"$prefix/share/licenses/"}
        mkdir -p "$stage/share/licenses/$(dirname "$rel")"
        cp "$f" "$stage/share/licenses/$rel"
        count=$((count + 1))
    done <<<"$files"
    if [ "$count" -gt 0 ]; then
        echo "    $pkg: $count license files ($license)"
        continue
    fi
    short=${pkg#mingw-w64-ucrt-x86_64-}
    text=${fallback_text[$short]:-}
    [ -n "$text" ] || die "$pkg ($license) installs no license file; add it to fallback_text"
    [ "${license#spdx:}" = "${fallback_license[$short]}" ] ||
        die "$pkg is now licensed '$license', not '${fallback_license[$short]}'; check its fallback_text"
    [ -f "$text" ] || die "$text (license text for $pkg) does not exist"
    grep -q 'GNU LESSER GENERAL PUBLIC LICENSE' "$text" && grep -q 'Version 2\.1' "$text" ||
        die "$text is not the LGPL-2.1 text"
    text_pkg=$(pacman -Qqo "$text") || die "pacman cannot tell which package owns $text"
    mkdir -p "$stage/share/licenses/$short"
    cp "$text" "$stage/share/licenses/$short/COPYING"
    {
        echo "$pkg installs no license file. Its License field is $license."
        echo "COPYING here is the text of that license, copied from"
        echo "${text#"$prefix/"} in $text_pkg."
    } >"$stage/share/licenses/$short/NOTE.txt"
    echo "    $pkg: no license files; COPYING from $text_pkg ($license)"
done
{
    echo "Colorz for Windows bundles these MSYS2 packages. Their source code is at"
    echo "https://repo.msys2.org/mingw/sources/ and their build recipes at"
    echo "https://github.com/msys2/MINGW-packages."
    echo
    printf '%s\n' "${sources[@]}"
} >"$stage/share/licenses/SOURCES.txt"
cp "$repo/LICENSE" "$repo/README.md" "$stage/share/licenses/colorz/"

echo "==> Staged $stage"
echo "    ${#packages[@]} packages, $(find "$stage" -type f | wc -l) files, $(du -sh "$stage" | cut -f1)"
