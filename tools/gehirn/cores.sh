#!/bin/sh
# GEHIRN — build the pinned aarch64 libretro core set.
#
# usage: tools/gehirn/cores.sh [outdir]
#
# Runs in CI (debian:bullseye + gcc-aarch64-linux-gnu), same toolchain family
# as the frontend, so every core keeps the glibc floor low. Every core is
# pinned to a commit below; bump a pin deliberately, never implicitly.
# GPL cores also ship their corresponding source archive beside the binary.
# Output: <outdir>/System/magi/cores/*.so, licenses/<name>/, licenses/README.txt

set -u

OUT=${1:-dist-cores}
CROSS=${CROSS:-aarch64-linux-gnu-}
JOBS=${JOBS:-$(nproc 2>/dev/null || echo 4)}
WORK=$(mktemp -d)
FAILED=""

# name|repo|commit|build-kind|ship-source  (name == systems.json "core" value)
# ship-source is 1 for GPL cores: the pinned source archive rides along in
# licenses/ as the corresponding source.
CORES='gambatte|https://github.com/libretro/gambatte-libretro|d9d6cd06382d1ced30de34d56d3609452323dab1|make|1
mgba|https://github.com/libretro/mgba|7a12d6d4b9acb14c0ae62c9166b6a2f3d08007f6|mgba|0
fceumm|https://github.com/libretro/libretro-fceumm|ac0d00a7fc01166f84c7bac01d1084ebfd2b865e|make|1
snes9x2005_plus|https://github.com/libretro/snes9x2005|a79dfe9047e7fec58808aefe48ad2bf499c7af11|make|0
genesis_plus_gx|https://github.com/libretro/Genesis-Plus-GX|aa74d79634cd230f65cce4180e92688e53883d2f|gpgx|0
flycast|https://github.com/libretro/flycast|4a913e063c95d1fae98afc64645831de0bcad57e|flycast|1
mednafen_ngp|https://github.com/libretro/beetle-ngp-libretro|a50d5ac288a81f2104ddf43195a4efdd15c72227|make|1
pokemini|https://github.com/libretro/PokeMini|132111b76343559860532a1ccc094f93f1ed5650|make|1
mednafen_pce|https://github.com/libretro/beetle-pce-libretro|b96c11e095b6a40a412d2da02766ae1c3f4fd539|make|1
pcsx_rearmed|https://github.com/libretro/pcsx_rearmed|c8816799b50388e61cfe237fe2cdbb7d8175f20a|pcsx|1'

fetch() { # repo commit dir
    git init -q "$3"
    git -C "$3" remote add origin "$1"
    git -C "$3" fetch -q --depth 1 origin "$2"
    git -C "$3" checkout -q FETCH_HEAD
    # Some cores carry submodules (pcsx_rearmed needs libpicofe).
    git -C "$3" submodule update --init --recursive --depth 1 >/dev/null 2>&1 \
        || git -C "$3" submodule update --init --recursive >/dev/null 2>&1 || true
    # Shallow clones carry no tags; some builds (mGBA) run git describe for
    # their version string, so plant a plausible one.
    git -C "$3" tag -f v0.11.0 >/dev/null 2>&1 || true
}

build_core() { # dir kind commit
    case "$2" in
        mgba)
            # mGBA has no makefile: the libretro core is a CMake target.
            # Only the libretro target is built, and it links OS_LIB only —
            # zlib must therefore resolve to the system library (the
            # container carries the arm64 zlib; the device ships libz.so.1).
            # With the bundled zlib fallback, HAVE_CRC32 is still defined
            # while zlibstatic is never linked, leaving crc32 undefined.
            ( cd "$1" && cmake -S . -B build \
                -DLIBMGBA_ONLY=ON \
                -DBUILD_LIBRETRO=ON \
                -DZLIB_LIBRARY=/usr/lib/aarch64-linux-gnu/libz.so \
                -DZLIB_INCLUDE_DIR=/usr/include \
                -DCMAKE_BUILD_TYPE=Release \
                -DCMAKE_SYSTEM_NAME=Linux \
                -DCMAKE_SYSTEM_PROCESSOR=aarch64 \
                -DCMAKE_C_COMPILER="${CROSS}gcc" \
                -DCMAKE_CXX_COMPILER="${CROSS}g++" \
              && cmake --build build --target mgba_libretro -j "$JOBS" ) 2>&1
            ;;
        gpgx)
            ( cd "$1" && make -j"$JOBS" -f Makefile.libretro platform=unix \
                CC="${CROSS}gcc" CXX="${CROSS}g++" AR="${CROSS}ar" ) 2>&1
            ;;
        pcsx)
            # No ./configure: Makefile.libretro sets NO_CONFIG_MAK and takes
            # its configuration as variables. HAVE_PHYSICAL_CDROM=0 skips the
            # physical-CD path (there is no drive), and the aarch64 dynarec is
            # ari64. CFLAGS goes in through the environment so the makefile
            # can still append to it, and GIT_VERSION keeps the makefile off
            # git. platform=unix pieces are appended by the wrapper makefile.
            ( cd "$1" \
              && CFLAGS="-mcpu=cortex-a53" make -j"$JOBS" -f Makefile.libretro platform=unix \
                    DYNAREC=ari64 BUILTIN_GPU=neon HAVE_CHD=1 HAVE_PHYSICAL_CDROM=0 \
                    CC="${CROSS}gcc" CXX="${CROSS}g++" AR="${CROSS}ar" \
                    GIT_VERSION="\" $(printf %s "$3" | cut -c1-7)\"" ) 2>&1
            ;;
        flycast)
            # Knulli ships this 2021 commit as flycastvl on H700: the older
            # GLES2 renderer is much lighter than the current one. The
            # mali-drm-gles2 target is the aarch64 GLES2 build (Cortex-A53,
            # ARM64 dynarec); ARCH=arm64 makes the makefile use the cross
            # gcc as the assembler for ngen_arm64.S. OpenMP and xBRZ are left
            # out to avoid a libgomp dependency; the modem/DCNet build is not
            # needed. Threaded rendering, CHD images and NVMEM saves are kept.
            ( cd "$1" && make -j"$JOBS" platform=mali-drm-gles2 \
                ARCH=arm64 HAVE_OPENMP=0 HAVE_TEXUPSCALE=0 ENABLE_MODEM=0 \
                HAVE_CDROM=0 HAVE_CHD=1 NO_THREADS=0 NO_NVMEM=0 \
                CC="${CROSS}gcc" CXX="${CROSS}g++" AR="${CROSS}ar" ) 2>&1
            ;;
        *)
            ( cd "$1" && make -j"$JOBS" platform=unix \
                CC="${CROSS}gcc" CXX="${CROSS}g++" AR="${CROSS}ar" ) 2>&1
            ;;
    esac
}

run_core() { # name repo commit kind src
    name=$1 repo=$2 commit=$3 kind=$4 src=$5
    echo "== $name @ $commit"
    dir="$WORK/$name"
    if ! fetch "$repo" "$commit" "$dir" >"$WORK/$name.fetch.log" 2>&1; then
        echo "!! $name: fetch failed"; tail -n 10 "$WORK/$name.fetch.log"
        FAILED="$FAILED $name"; return
    fi
    if ! build_core "$dir" "$kind" "$commit" >"$WORK/$name.build.log" 2>&1; then
        echo "!! $name: build failed; last lines:"
        tail -n 40 "$WORK/$name.build.log"
        FAILED="$FAILED $name"; return
    fi
    so=$(find "$dir" -maxdepth 1 -name '*_libretro.so' -print -quit)
    [ -n "$so" ] || so=$(find "$dir" -name '*_libretro.so' -print -quit)
    if [ -z "$so" ]; then
        echo "!! $name: no *_libretro.so produced"
        FAILED="$FAILED $name"; return
    fi
    mkdir -p "$OUT/System/magi/cores/licenses/$name"
    cp "$so" "$OUT/System/magi/cores/${name}_libretro.so"
    ${CROSS}strip --strip-unneeded "$OUT/System/magi/cores/${name}_libretro.so" 2>/dev/null || true
    # Undefined symbols kill the process at first use on the device (glibc
    # exits 127), so nothing may rely on symbols no shipped library provides.
    # Every import is listed for review; crc32 is the known trap.
    und=$(${CROSS}objdump -T "$OUT/System/magi/cores/${name}_libretro.so" 2>/dev/null \
        | awk '$3=="*UND*" {print $NF}' | sort -u | tr '\n' ' ')
    echo "-- $name: imports: ${und:-none}"
    if echo " $und" | grep -q ' crc32 '; then
        if ${CROSS}objdump -p "$OUT/System/magi/cores/${name}_libretro.so" 2>/dev/null \
            | grep -q 'NEEDED.*libz\.so'; then
            echo "-- $name: crc32 is provided by libz"
        else
            echo "!! $name: core references an unprovided crc32 symbol (zlib false positive?)"
            FAILED="$FAILED $name:undef"
        fi
    fi
    for lic in "$dir"/COPYING* "$dir"/LICENSE* "$dir"/LICENCE* "$dir"/license* "$dir"/LICENSES*; do
        [ -e "$lic" ] && cp -r "$lic" "$OUT/System/magi/cores/licenses/$name/" 2>/dev/null
    done
    if [ "$src" = "1" ]; then
        slug=${repo#https://github.com/}
        if curl -fsSL -o "$OUT/System/magi/cores/licenses/$name/source-$commit.tar.gz" \
            "https://github.com/$slug/archive/$commit.tar.gz"; then
            echo "-- $name: corresponding source archived"
        else
            echo "!! $name: could not fetch corresponding source"
            FAILED="$FAILED $name:src"
        fi
    fi
    echo "-- $name: ok, $(wc -c <"$OUT/System/magi/cores/${name}_libretro.so") bytes"
}

mkdir -p "$OUT/System/magi/cores/licenses"

oldifs=$IFS
IFS='
'
for row in $CORES; do
    IFS=$oldifs
    set -- $(printf '%s' "$row" | tr '|' ' ')
    [ $# -eq 5 ] && run_core "$1" "$2" "$3" "$4" "$5"
    IFS='
'
done
IFS=$oldifs

# Provenance for the shipped binaries.
{
    echo "MAGI cores — built from pinned sources for aarch64 (glibc <= 2.31)"
    echo
    echo "Each core below is compiled from its public repository at the noted"
    echo "commit. License texts are in licenses/<name>/. GPL cores also ship"
    echo "their corresponding source archive (source-<commit>.tar.gz)."
    echo
    echo "$CORES" | while IFS='|' read -r name repo commit kind src; do
        echo "  $name  $repo  $commit"
    done
} >"$OUT/System/magi/cores/licenses/README.txt"

echo "== glibc symbol floors"
for so in "$OUT"/System/magi/cores/*.so; do
    [ -e "$so" ] || continue
    vers=$(${CROSS}objdump -T "$so" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -Vu | tail -1)
    echo "  $(basename "$so"): ${vers:-none}"
done

if [ -n "$FAILED" ]; then
    echo "GEHIRN: cores failed:$FAILED" >&2
    exit 1
fi
echo "GEHIRN: core set ready in $OUT/System/magi/cores/"
