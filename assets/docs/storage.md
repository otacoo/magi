# SD card repair

An interrupted write, a reset press or a power cut can leave the card's FAT
chain damaged. The kernel then remounts the card read-only, so saves, states,
settings and screenshots fail even though the card looks writable on a PC.

MAGI checks the mount on the shelf and at game launch. When the card is
read-only it shows a full-screen alert:

- **SELECT+START** opens the repair question.
- **A** starts the repair.
- **B** dismisses the alert and continues without saving.

A failed state save in the in-game pause menu says **CARD READ-ONLY** instead
of the generic **SAVE FAILED**. Exit the game to reach the shelf alert.

## What the repair does

The frontend writes a small repair script to `/tmp`, then replaces itself with
it. This releases the card so it can be unmounted. The script:

1. unmounts `/mnt/sdcard`;
2. runs `fsck.fat -a` on a FAT32 card, or `fsck.exfat -y` when that tool is
   installed;
3. writes progress and the result to `/data/magi-repair.log` and
   `/data/magi-repair.result`;
4. reboots the device.

The next boot reports the result on the shelf. BaseOS currently ships
`fsck.fat`; an exFAT card without `fsck.exfat` shows **REPAIR NEEDS A COMPUTER**
instead. Back up important files before repairing: fsck may move or drop
damaged entries.
