#!/bin/sh
# MAGI card repair. Runs in place of the frontend after it execs this script.
# args: <device> <mount point> <filesystem>

DEV=${1:-}
MNT=${2:-/mnt/sdcard}
FS=${3:-}

LOG=/data/magi-repair.log
RESULT=/data/magi-repair.result
OUT=/data/magi-repair.out
SPLASH=/usr/bin/baseos-splash
[ -x "$SPLASH" ] || SPLASH=/bin/baseos-splash
FSCK_FAT=/sbin/fsck.fat
FSCK_EXFAT=/sbin/fsck.exfat
[ -x /usr/sbin/fsck.fat ] && FSCK_FAT=/usr/sbin/fsck.fat
[ -x /usr/sbin/fsck.exfat ] && FSCK_EXFAT=/usr/sbin/fsck.exfat
[ -x /bin/fsck.fat ] && FSCK_FAT=/bin/fsck.fat
[ -x /bin/fsck.exfat ] && FSCK_EXFAT=/bin/fsck.exfat

cd / || exit 1
: >"$OUT" 2>/dev/null || {
	OUT=/tmp/magi-repair.out
	: >"$OUT" 2>/dev/null || OUT=/dev/null
}
exec >>"$OUT" 2>&1 </dev/null

log() {
	echo "$(date '+%F %T') $*" >>"$LOG" 2>/dev/null
}

say() {
	[ -x "$SPLASH" ] && "$SPLASH" --important -1 "$1" >/dev/null 2>&1
}

limited() {
	limit=$1
	shift
	if command -v timeout >/dev/null 2>&1; then
		timeout "$limit" "$@"
	else
		"$@"
	fi
}

finish() {
	status=$1
	if ! printf '%s\n' "$status" >"$RESULT" 2>/dev/null; then
		printf '%s\n' "$status" >/tmp/magi-repair.result 2>/dev/null
	fi
	log "result $status"
	limited 30 sync
	if [ "$status" = repaired ] || [ "$status" = clean ]; then
		say "REPAIR COMPLETE"
	else
		say "REPAIR FAILED: USE A COMPUTER"
	fi
	sleep 1
	say "REBOOTING"
	sleep 2
	limited 30 sync
	reboot 2>/dev/null || reboot -f 2>/dev/null || poweroff 2>/dev/null
	exit 0
}

if [ -z "$DEV" ]; then
	log "repair: no device"
	finish failed
fi

log "repair $DEV $MNT $FS"
case "$FS" in
	vfat) FSCK=$FSCK_FAT; FIX=-a ;;
	exfat) FSCK=$FSCK_EXFAT; FIX=-y ;;
	*)
		log "repair: unsupported filesystem $FS"
		finish unsupported
		;;
esac
if [ ! -x "$FSCK" ]; then
	log "repair: no $FSCK"
	finish failed
fi

say "UNMOUNTING SD CARD"
limited 30 sync
if ! umount "$MNT" 2>/dev/null; then
	if command -v fuser >/dev/null 2>&1; then
		fuser -km "$MNT" >/dev/null 2>&1
	fi
	sleep 1
	if ! umount "$MNT" 2>/dev/null; then
		log "repair: cannot unmount $MNT"
		finish failed
	fi
fi
if grep -q " $MNT " /proc/mounts 2>/dev/null; then
	log "repair: $MNT is still mounted"
	finish failed
fi

say "REPAIRING SD CARD"
pass=0
first=0
code=1
while [ "$pass" -lt 6 ]; do
	pass=$((pass + 1))
	limited 600 "$FSCK" "$FIX" "$DEV" >>"$LOG" 2>&1
	code=$?
	[ "$pass" -eq 1 ] && first=$code
	log "$(basename "$FSCK") $FIX $DEV pass $pass exit $code"
	[ "$code" -eq 0 ] && break
	[ "$code" -eq 124 ] && break
done

if [ "$code" -eq 0 ]; then
	if [ "$first" -eq 0 ]; then
		finish clean
	else
		finish repaired
	fi
fi
finish failed
