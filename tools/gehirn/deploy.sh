#!/bin/sh
# GEHIRN — deploy a MAGI build to a running console and reboot it.
#
# usage: tools/gehirn/deploy.sh <adb|user@host> [binary]
#
#   tools/gehirn/deploy.sh adb                  # over USB (adbd is on by default)
#   tools/gehirn/deploy.sh root@rg35xxsp.local  # over Wi-Fi (BaseOS defaults)
#
# The binary is swapped in atomically (push to a temp name, rename over the
# target — the running frontend keeps its old inode) and the console is
# rebooted into the new build. SD defaults to /mnt/sdcard.

set -eu

TARGET=${1:?usage: tools/gehirn/deploy.sh <adb|user@host> [binary]}
BIN=${2:-target/aarch64-unknown-linux-gnu/release/magi}
SD=${SD:-/mnt/sdcard}

# Prefer a repo-local adb (platform-tools dropped in adb/ at the root,
# git-excluded) before relying on PATH.
ADB=${ADB:-adb}
if ! command -v "$ADB" >/dev/null 2>&1; then
    ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
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
        "$ADB" push "$BIN" "$SD/System/frontend.new"
        "$ADB" shell "mv -f '$SD/System/frontend.new' '$SD/System/frontend' && sync"
        "$ADB" reboot
        echo "GEHIRN: pushed over adb; the console is rebooting into the new build."
        ;;
    *@*)
        scp "$BIN" "$TARGET:$SD/System/frontend.new"
        ssh "$TARGET" "mv -f '$SD/System/frontend.new' '$SD/System/frontend' && sync && reboot"
        echo "GEHIRN: pushed to $TARGET; the console is rebooting into the new build."
        ;;
    *)
        echo "GEHIRN: target must be 'adb' or user@host" >&2
        exit 2
        ;;
esac
