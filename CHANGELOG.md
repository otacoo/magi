# Changelog

All notable changes to MAGI are documented here.

## [Unreleased]

### Added

- Game Boy and Game Boy Color netplay over a local hotspot or Wi-Fi, with a
  four-character host code and each player keeping their own save.
- A redesigned in-game pause menu with folder tabs, direct save/load slots,
  full-size state previews and inline core/video options.
- SCREENSHOTS below the screenshot toggle, shown while it is on, to browse
  captures on the card and zoom them full-screen.

## [1.0.0] - 2026-10-27

First stable release, targeting BaseOS 1.5.x on the Anbernic H700 family.

### Added

- Shelf with a system rail, favorites, resume, themes, per-system overlays,
  a clock, battery state and power management.
- Shelf settings: SIMPLE/ART interface, auto art, art type, art background and
  manual download; box art from libretro-thumbnails.
- FAVORITES tab and starred games from every folder.
- User themes and a per-element colour palette.
- libretro host with save states, rewind, fast-forward, integer scaling,
  screenshots and an in-game OSD.
- Multi-disc PlayStation via `.m3u`, with SWAP DISC in the pause menu.
- Native N64 ports with automatic discovery, save states, an OSD and a
  performance governor.
- Wi-Fi, Bluetooth controllers and audio, SSH, NTP and a hostname editor.
- Lid sleep and wake, timed and low-battery shutdown, charge limit and an
  abortable POWER-hold shutdown countdown.
- GPU presentation: scaling, bezel/CRT overlays, warmth and the in-game
  overlays run on the Mali GPU, with the framebuffer path as the fallback.
- Safe mode: repeated fast restarts start on defaults instead of looping.
- 640x480 and 720x480 payloads for the BaseOS device set, with 720x480 UI,
  CRT and bezel art; GBA uses the full 3x screen without a bezel.
- Analog sticks when the device has them: PS1 analog sticks plus a d-pad
  fallback for other cores.
- 12/24-hour clock, toggled with X on the CLOCK page; the shelf clock follows
  the setting and it persists.
- `magi --probe` reports the panel mode and analog axis ranges for hardware
  validation.
- BSD 2-Clause licensing with third-party attributions.

### Changed

- New cards start on the ART shelf with AUTO ART off and shipped CRT/bezel
  overlays for the 2D systems.
- The boot splash is 5 s and starts with the first visible frame; the ignition
  track ends with it.
- Per-process logs live in `Logs/` and cap at 256 KB.
