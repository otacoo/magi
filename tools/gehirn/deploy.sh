#!/bin/sh
# GEHIRN — deploy a MAGI build to a running console and reboot it.
#
# usage: tools/gehirn/deploy.sh <adb|user@host> [binary]
#
#   tools/gehirn/deploy.sh adb                  # over USB (adbd is on by default)
#   tools/gehirn/deploy.sh root@rg35xxsp.local  # over Wi-Fi (BaseOS defaults)
#
# The binary and boot logos are swapped in atomically (push to a temp name,
# rename over the target) and the console is rebooted into the new build.
# SD defaults to /mnt/sdcard.

set -eu

TARGET=${1:?usage: tools/gehirn/deploy.sh <adb|user@host> [binary]}
BIN=${2:-target/aarch64-unknown-linux-gnu/release/magi}
SD=${SD:-/mnt/sdcard}

# Prefer a repo-local adb (platform-tools dropped in adb/ at the root,
# git-excluded) before relying on PATH.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
ADB=${ADB:-adb}
if ! command -v "$ADB" >/dev/null 2>&1; then
    if [ -x "$ROOT/adb/adb.exe" ]; then
        ADB="$ROOT/adb/adb.exe"
    fi
fi

if [ ! -f "$BIN" ]; then
    echo "GEHIRN: no binary at $BIN (build one first)" >&2
    exit 1
fi

case "$TARGET" in
    adb)
        "$ADB" wait-for-device
        for panel in 640x480 720x480; do
            src="$ROOT/assets/boot/bootlogo-$panel.bmp"
            [ -f "$src" ] || continue
            "$ADB" push "$src" "$SD/System/bootlogo-$panel.bmp.new"
            "$ADB" shell "mv -f '$SD/System/bootlogo-$panel.bmp.new' '$SD/System/bootlogo-$panel.bmp'"
        done
        src="$ROOT/assets/boot/gehirn.png"
        if [ -f "$src" ]; then
            "$ADB" push "$src" "$SD/System/magi/assets/boot/gehirn.png.new"
            "$ADB" shell "mv -f '$SD/System/magi/assets/boot/gehirn.png.new' '$SD/System/magi/assets/boot/gehirn.png'"
        fi
        "$ADB" push "$BIN" "$SD/System/frontend.new"
        "$ADB" shell "mv -f '$SD/System/frontend.new' '$SD/System/frontend' && sync"
        "$ADB" reboot
        echo "GEHIRN: pushed over adb; the console is rebooting into the new build."
        ;;
    *@*)
        for panel in 640x480 720x480; do
            src="$ROOT/assets/boot/bootlogo-$panel.bmp"
            [ -f "$src" ] || continue
            scp "$src" "$TARGET:$SD/System/bootlogo-$panel.bmp.new"
            ssh "$TARGET" "mv -f '$SD/System/bootlogo-$panel.bmp.new' '$SD/System/bootlogo-$panel.bmp'"
        done
        src="$ROOT/assets/boot/gehirn.png"
        if [ -f "$src" ]; then
            scp "$src" "$TARGET:$SD/System/magi/assets/boot/gehirn.png.new"
            ssh "$TARGET" "mv -f '$SD/System/magi/assets/boot/gehirn.png.new' '$SD/System/magi/assets/boot/gehirn.png'"
        fi
        scp "$BIN" "$TARGET:$SD/System/frontend.new"
        ssh "$TARGET" "mv -f '$SD/System/frontend.new' '$SD/System/frontend' && sync && reboot"
        echo "GEHIRN: pushed to $TARGET; the console is rebooting into the new build."
        ;;
    *)
        echo "GEHIRN: target must be 'adb' or user@host" >&2
        exit 2
        ;;
esac
