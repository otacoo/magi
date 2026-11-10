# Changelog

All notable changes to MAGI are documented here.

## [Unreleased]

### Changes

- Added **Pokemon Mini** support with the PokeMini core, including its LCD, palette and rumble options, plus default bezels
- Added **Sega Dreamcast** support with the Flycast core, including analog-stick detection and digital triggers
- Dreamcast uses the lighter **Flycast VL** core for better speed
- Dreamcast renders at native 1x resolution; the Internal Resolution option is gone
- Dreamcast triggers use **L1/R1**; **L2/R2** stay with the frontend
- Dreamcast rumble is off by default (enable **Purupuru** in the CORE menu) and now fires short pulses only on spikes, not continuously
- Added a per-system **SWAP L1/R1** option, used by Dreamcast
- Added **DPAD AS ANALOG** for Dreamcast and PlayStation games that need a stick; it maps the d-pad to the left analog stick and is on by default for Dreamcast
- Dreamcast PAL games now run at their real 50 Hz refresh rate instead of fighting the 60 Hz frame clock
- Dreamcast save states require **Threaded Rendering** to be off; MAGI blocks them while it is on to avoid crashes
- A read-only or failing card no longer makes the shelf retry its settings save every frame, and is rechecked while the shelf is open
- The keyboard has **:**, **!**, **+** and **&** on the X shift layer
- Shelf art settings now use **AUTO DOWNLOAD ART**, and the redundant **DOWNLOAD ART** row is gone
- Dreamcast art now uses the Redump DAT; art lookup ignores punctuation and version noise such as `v1.01` or `Rev 1` and falls back to fuzzy DAT names
- The Card Tool can rename a ROM file, and clicking the cover preview opens the image picker
- Payload pushes keep the card's settings, saves and BIOS files
- Added per-game **play time** tracking in the shelf game menu, shown as hours:minutes:seconds
- Redesigned the shelf game menu with **INFO**, **GAME**, **SYSTEM** and **CLOSE** tabs
- Shelf game menu now uses the same d-pad tab and pane navigation as the in-game menu
- Added a per-system **SWAP A/B** option to the shelf game menu
- Swapped the shelf buttons: **X** opens the game menu and **Y** favorites a game

## [1.2.1] - 2026-11-02

### Changes

- Much improved **ART** shelf mode
   - Better tabs
   - Better in-game menus
   - Better animations
   - Better boot sequences and audio
- Improved shaders and overlays
   - Added a new LCD ACCURATE shader to better mimic GB and GBC screens
      - Only available for GB/GBC/NGP/NGPC
      - When set, it will hide certain core options and set screen to SCALE INTEGER automatically
   - Added **GAME BOY Light** overlay from [drkhrse's Miyoo Bezels](https://github.com/drkhrse/drkhrse_miyoo_bezels)
   - Added *no grid* versions of certain overlays
- **Headphones** will no longer pop on game start/end and will show an icon in the header while in use
- Charge Limit actually works now and stops charge to the set percentage
- Added a new theme colour: UNIT-02
- Improved defaults and available options for all cores
- Keyboard insert cursor can now be moved with L/R buttons
- Added a **CHECK FOR UPDATES** menu to update MAGI over the Internet
- Added mitigations and a way to repair the SD card when it becomes **READ ONLY**, see [SD card repair](assets/docs/storage.md)
- Removed ALL option for art
- Removed the last step from the BRIGHTNESS overlay that fully darken the screen
- Dropped support for NEOCD and PSP

### Commits
- ONE PAYLOAD (9925e3d)
- INTERLINKED (ba85d32)
- COLOR ACCURATE (8af5425)
- LCD (a0c18db)
- AKAGI (8006008)
- KEYS (3df680f)
- CHARGE LIMIT (a276068)
- PS1 DEFAULTS (d4d189e)
- NAOKO (f024d0d)
- CORE ROWS (2adf45e)
- UNIT-02 (ef0c58c)
- GAME BOY (ef95346)
- BOOT AUDIO (63fef99)
- ART SIZE (064ec1b)
- SHADERS (2d5d24a)
- NAMAE (daf81be)
- WORDS (8e4dcda)
- DROP PSP (20f8291)
- TWEAKS (20f0c28)
- ~~PSP (d77dd12)~~
- DROP NEOCD (de79218)
- HEADPHONES (69cb65e)
- SOUND ONLY (8cd9e88)
- READ ONLY (ab5f4ea)
- TAB TECH (18e3240)
- STATIC TOOLING (458b8b2)


## [1.1.0] - 2026-10-28

### Changes

- REFINE (cecb4d2)
- GENESIS (ec10c9e)
- INTERMISSION 2 (e6ea728)
- INTERMISSION (20bebc4)
- SHOTS (0a68788)
- SPLIT (8041de2)
- SCHEDULER (57dfa62)
- PLUG ENTRY (7472a6a)
- GEHIRN OPTIMIERUNG (77a9860)

### Added

- Game Boy and Game Boy Color *netplay* over a local hotspot or Wi-Fi, with a
  four-character host code and each player keeping their own save.
- A *redesigned in-game pause menu* with folder tabs, direct save/load slots,
  full-size state previews and inline core options.
- Added a SCREENSHOTS page below the SCREENSHOT toggle, to browse captures on the card.

### Changed

**Genesis:**
- Genesis will now use the schedutil governor, which should reduce battery usage.
- Genesis FM, per-channel FM volume, preamp, CD-DA and PCM options have been hidden from the core menu.

**Pause Menu:**
- Long core option labels and values scroll instead of truncating.
- The pause menu remembers the last tab used during the game.
- The hold-MENU save-and-exit prompt is a larger, clearer progress panel.

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
