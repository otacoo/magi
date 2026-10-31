#!/bin/sh
# GEHIRN — compose the MAGI card payload.
#
# usage: tools/gehirn/package.sh <path-to-aarch64-magi> [outdir] [coresdir] [panel]
#
# panel is 640x480 (4:3) or 720x480 (3:2). It selects the bootlogo and overlay
# art and writes System/magi/panel. Devices themselves are detected at runtime
# through BaseOS, the same set BaseOS supports.
# coresdir is an optional System/magi/cores tree (from cores.sh, built in CI);
# its .so files and licenses are merged into the payload. Without it the
# payload keeps the empty cores/ folder and its README.
#
# The result is a directory tree whose contents are copied to the root of the
# frontend card. BaseOS finds the binary at System/frontend (no executable bit
# needed — it runs through its loader). Folders carry a README.txt so GitHub
# artifact zips keep them (empty directories are dropped).

set -eu

BIN=${1:?usage: tools/gehirn/package.sh <path-to-aarch64-magi> [outdir] [coresdir] [panel]}
OUT=${2:-dist}
CORES=${3:-}
PANEL=${4:-640x480}

case "$PANEL" in
    640x480|720x480) ;;
    *)
        echo "GEHIRN: unknown panel: $PANEL (use 640x480 or 720x480)" >&2
        exit 1
        ;;
esac

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)

if [ ! -f "$BIN" ]; then
    echo "GEHIRN: no such binary: $BIN" >&2
    exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT/System/magi/assets" "$OUT/System/magi/cores" \
         "$OUT/Config" "$OUT/BIOS" "$OUT/Overlays" "$OUT/Saves" \
         "$OUT/Screenshots" "$OUT/Logs" "$OUT/Games" "$OUT/Themes"

cp "$BIN" "$OUT/System/frontend"
cp "$ROOT/assets/systems.json" "$OUT/System/magi/systems.json"
cp "$ROOT/assets/fonts/OFL.txt" "$OUT/System/magi/assets/OFL.txt"
cp "$ROOT/assets/themes/_template.ini" "$OUT/Themes/_template.ini"
cp -r "$ROOT/assets/overlays/." "$OUT/Overlays/"
# RetroArch overlay configs are for RetroArch; MAGI reads the PNGs only.
find "$OUT/Overlays" -name '*.cfg' -delete
if [ "$PANEL" = 640x480 ]; then
    find "$OUT/Overlays" -name '*720x480*' -delete
fi
printf '%s\n' "$PANEL" > "$OUT/System/magi/panel"
mkdir -p "$OUT/System/magi/assets/boot"
cp "$ROOT/assets/boot/gehirn.png" "$OUT/System/magi/assets/boot/gehirn.png"
cp "$ROOT/assets/boot/ignition.wav" "$OUT/System/magi/assets/boot/ignition.wav"
cp "$ROOT/assets/boot/SOURCE.md" "$OUT/System/magi/assets/boot/SOURCE.md"
# SDL's community controller database (loaded explicitly by pads.rs).
mkdir -p "$OUT/System/magi/controllers"
cp "$ROOT/assets/controllers/gamecontrollerdb.txt" "$OUT/System/magi/controllers/gamecontrollerdb.txt"
cp "$ROOT/assets/controllers/SOURCE.md" "$OUT/System/magi/controllers/SOURCE.md"
cp "$ROOT/assets/controllers/LICENSE" "$OUT/System/magi/controllers/LICENSE"
# BaseOS draws this before the frontend starts (hides the vendor splash).
cp "$ROOT/assets/boot/bootlogo-$PANEL.bmp" "$OUT/System/bootlogo-$PANEL.bmp"
if [ "$PANEL" = 720x480 ]; then
    # The partition may still carry a 640x480 logo on an earlier image.
    cp "$ROOT/assets/boot/bootlogo-640x480.bmp" "$OUT/System/bootlogo-640x480.bmp"
fi
# N64 native ports: the Miyoo SDL 1.2 sysroot build and the fb/input/ALSA shim.
mkdir -p "$OUT/System/magi/n64"
cp "$ROOT/assets/n64/libmiyoofb.so" "$OUT/System/magi/n64/libmiyoofb.so"
cp "$ROOT/assets/n64/libn64path.so" "$OUT/System/magi/n64/libn64path.so"
cp "$ROOT/assets/n64/libSDL-1.2.so.0" "$OUT/System/magi/n64/libSDL-1.2.so.0"
cp "$ROOT/assets/n64/SOURCE.md" "$OUT/System/magi/n64/SOURCE.md"
cp "$ROOT/assets/n64/LICENSE-SDL" "$OUT/System/magi/n64/LICENSE-SDL"

cat > "$OUT/System/magi/README.txt" <<'EOF'
MAGI
====
System/frontend            the MAGI binary — BaseOS runs it automatically
System/magi/systems.json   system -> folder -> extensions -> core mapping
System/magi/cores/         libretro cores (aarch64, BaseOS glibc runtime)
Logs/                      one capped log per process (diagnostics)

Card folders: Games/<CONSOLE>/ for games, Config/ for user settings,
BIOS/, Overlays/, Saves/, Themes/. Add a system by appending to systems.json
and dropping its core in cores/. Add a theme by copying Themes/_template.ini
to a new name ending in .ini and editing its colours.

N64 native ports: copy the port's App/<Game>/ folder into Games/N64/<Game>/
and put your own ROM beside it as rom.z64 (so Games/N64/<Game>/rom.z64).

EOF

cat > "$OUT/System/magi/cores/README.txt" <<'EOF'
Drop libretro cores here (aarch64, built for glibc <= 2.35), named like
mgba_libretro.so or mgba.so. systems.json refers to them by stem.
EOF

if [ -n "$CORES" ] && [ -d "$CORES" ]; then
    cp -r "$CORES"/. "$OUT/System/magi/cores/"
    echo "GEHIRN: cores merged from $CORES"
fi

cat > "$OUT/Games/README.txt" <<'EOF'
Games, one folder per console: GB, GBC, GBA, NES, SNES, GENESIS, PS1, NGPC,
PCE, ...
Folder names are declared in System/magi/systems.json ("folder").
EOF

cat > "$OUT/Config/README.txt" <<'EOF'
User configuration lives here (e.g. a systems.json override).
EOF

cat > "$OUT/BIOS/README.txt" <<'EOF'
Console BIOS files (for systems that need one):
  GBA: gba_bios.bin (gba_bios.gba is copied to it)
  PS1: scph5501.bin (US), scph5500.bin (Japan), scph5502.bin (Europe)
  PC Engine CD: syscard3.pce, syscard3u.pce (US), syscard2/1.pce, gexpress.pce
                (HuCards do not use a CD System Card)
EOF

cat > "$OUT/Overlays/README.txt" <<'EOF'
Screen overlays for the in-game OVERLAYS page (two layers: CRT, then
bezel - lowest first). PNGs at 640x480; the folders are scanned recursively
and sorted by name: folders or files with "crt" (e.g. CRT/) go to the CRT
layer, everything else is a bezel. Colour-correction files are ignored
(use the core options for that). RetroArch .cfg files are not used: the game
screen area comes from the PNG's transparent pixels, not from viewports.

The bundled set is "Perfect Overlays" by ourigen, derived from the work of
1playerinsertcoin with bezels by mugwomp_93 and drkhrse:
https://github.com/ourigen/Perfect_Overlays
EOF

cat > "$OUT/Saves/README.txt" <<'EOF'
Battery saves and save states, one folder per console.
EOF

cat > "$OUT/Screenshots/README.txt" <<'EOF'
Screenshots taken with SELECT+MENU+R1 land here (24-bit BMP).
EOF

cat > "$OUT/Logs/README.txt" <<'EOF'
One capped log per process: magi.log (shelf), magi-run.log (libretro host),
magi-n64-supervisor.log, magi-n64.log and magi-n64-aplay.log (N64 ports).
EOF

for f in $(sed -n 's/.*"folder": *"\([^"]*\)".*/\1/p' "$OUT/System/magi/systems.json"); do
    mkdir -p "$OUT/Games/$f"
    if [ "$f" = "N64" ]; then
        cat > "$OUT/Games/$f/README.txt" <<'EOF'
N64 native ports: copy a port's App/<Game>/ contents here as
Games/N64/<Game>/ and put your own matching ROM inside it as rom.z64.
Only the App/<Game>/ folder is needed; the zip's Apps/, Emus/ and Roms/
folders are Miyoo launcher metadata and are not used by MAGI. There is no
App/ folder on the card after the port is installed. MAGI finds the port's
executable and data directory from its launch.sh, so new ports need no
per-game entry. Start it from the N64 shelf; the port builds its data on
first launch.
In a port, VOLUME adjusts the sound; SELECT + D-PAD adjusts brightness and
temperature with the MAGI bar.
EOF
    elif [ "$f" = "PS1" ]; then
        cat > "$OUT/Games/$f/README.txt" <<'EOF'
PlayStation discs: .cue, .chd or .pbp. A multi-disc game is one .m3u listing
its disc files, one name per line, relative to this folder. The shelf shows
the playlist as one title and the pause menu gains SWAP DISC.
EOF
    else
        echo "Games for $f. Unzipped ROMs only." > "$OUT/Games/$f/README.txt"
    fi
done

echo "GEHIRN: payload ready in $OUT/"
echo "folders: System, Games, Config, BIOS, Overlays, Saves, Screenshots, Logs, Themes"
echo "copy the contents of $OUT to the root of your frontend card."
