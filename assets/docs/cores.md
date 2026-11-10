# Cores

The payload ships pinned aarch64 libretro cores in `System/magi/cores/`;
`System/magi/systems.json` maps each system to its folder, extensions and
core. Use unzipped ROMs.

| System | Folder | Core |
|---|---|---|
| Game Boy / Color | `GB` / `GBC` | `gambatte` |
| Game Boy Advance | `GBA` | `mgba` |
| NES / FDS | `NES` | `fceumm` |
| SNES | `SNES` | `snes9x2005_plus` |
| Genesis / Mega Drive | `GENESIS` | `genesis_plus_gx` |
| Dreamcast | `DC` | `flycast` |
| Neo Geo Pocket / Color | `NGPC` | `mednafen_ngp` |
| Pokemon Mini | `POKEMINI` | `pokemini` |
| PlayStation | `PS1` | `pcsx_rearmed` |
| PC Engine / SuperGrafx | `PCE` | `mednafen_pce` |
| Nintendo 64 | `N64` | native ports (see [Miyoo64 ports](n64-ports.md)) |

Per-system cores must be built for the device (aarch64, glibc 2.31 or older).
Adding one means dropping the `.so` into `System/magi/cores/` and adding an
entry to `systems.json`.

## BIOS files

MAGI loads BIOS files automatically from `BIOS/`:

- **GBA**: `gba_bios.bin` (`gba_bios.gba` is copied to that name).
- **PlayStation**: the BIOS matching the game's region — `scph5501.bin` (USA),
  `scph5500.bin` (Japan), `scph5502.bin` (Europe).
- **PC Engine CD**: `syscard3.pce` (or `syscard3u.pce` or another System
  Card). HuCards do not use a CD System Card.
- **Pokemon Mini**: `bios.min` (optional; the free BIOS is used when absent).
- **Dreamcast**: `BIOS/dc/dc_boot.bin` and `BIOS/dc/dc_flash.bin` (optional;
  the HLE BIOS is used when absent).

**PokeMini** exposes its video and rumble options in the CORE tab: Video Scale,
LCD Filter, LCD Mode, LCD Contrast, LCD Brightness, Palette, Piezo Filter,
Screen Shake Level and Controller Rumble Level. Set a rumble level to `0` to
turn that effect off. The pad maps **A/B** to A/B, **L1** to Shake, **R1** to C
and **SELECT** to the console's Power button.

**Flycast** is the 2021 **Flycast VL** build, which is lighter on the H700
GPU. It puts the Dreamcast's triggers on **L1/R1** (Digital Triggers is on by
default on H700 hardware); **L2/R2** stay with the frontend. The analog
deadzone rows appear only when a real stick is attached; the stick works on
devices that have one, and the d-pad keeps working everywhere. For games that
need a stick, turn on **DPAD AS ANALOG** in the shelf's SYSTEM tab.

Save states need **Threaded Rendering** off: Flycast VL cannot serialize
safely while its emulation thread is running. With threaded rendering on,
MAGI refuses state saves instead of crashing; VMU saves still work. State
saves work normally when threaded rendering is disabled.

Core options are edited in the in-game pause menu and persist per core in
`Config/core-options.json`.
