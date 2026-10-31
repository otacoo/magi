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
| Neo Geo Pocket / Color | `NGPC` | `mednafen_ngp` |
| PlayStation | `PS1` | `pcsx_rearmed` |
| PlayStation Portable | `PSP` | `ppsspp` |
| PC Engine / SuperGrafx | `PCE` | `mednafen_pce` |
| Nintendo 64 | `N64` | native ports (see [Miyoo64 ports](n64-ports.md)) |

PPSSPP renders on the Mali GPU through libretro hardware rendering. It falls
back to its software renderer automatically when the GLES presenter cannot
start (HDMI output or `MAGI_GPU=0`).

Per-system cores must be built for the device (aarch64, glibc 2.31 or older).
Adding one means dropping the `.so` into `System/magi/cores/` and adding an
entry to `systems.json`.

## BIOS files

MAGI loads BIOS files automatically from `BIOS/`:

- **GBA**: `gba_bios.bin` (`gba_bios.gba` is copied to that name).
- **PlayStation**: the BIOS matching the game's region — `scph5501.bin` (USA),
  `scph5500.bin` (Japan), `scph5502.bin` (Europe).
- **PlayStation Portable**: no BIOS required. Optional PPSSPP assets: copy
  the contents of PPSSPP's `assets/` folder directly into `BIOS/PPSSPP/`.
- **PC Engine CD**: `syscard3.pce` (or `syscard3u.pce` or another System
  Card). HuCards do not use a CD System Card.

Core options are edited in the in-game pause menu and persist per core in
`Config/core-options.json`.
