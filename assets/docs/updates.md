# Updates

**SETTINGS → SYSTEM → CHECK FOR UPDATES** fetches the latest MAGI release from
GitHub. It needs Wi-Fi, a writable card and at least 20% battery.

The updater downloads the payload for the panel it is running on, verifies its
SHA-256 checksum, unpacks it in `/tmp`, and installs the managed files:
`System/` and `Overlays/`, plus `Themes/_template.ini`. It does not touch
`Config/`, `Games/`, `Saves/`, `Screenshots/`, `BIOS/`, `Logs/` or your own
theme files.

When the screen says **UPDATE INSTALLED**, press **A** to restart. The new
version is used after the reboot. If anything fails, the screen shows the
reason and **B** returns to SETTINGS; files already written stay valid, so the
update can be retried.

Each file is written through the same temp-file-and-rename path as the rest of
MAGI. Do not power off while the install is running.
