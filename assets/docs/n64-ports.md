# Miyoo64 ports

MAGI can *only* run [Miyoo64-style](https://miyoo-n64.pages.dev/) 32-bit native
ports, not libretro N64 cores.

1. Download the `.zip` of a port from the Miyoo64 page.
2. Open its `App` folder and copy the folder with the game name to
   `Games/N64/` on the card.
3. The Miyoo launcher's `Apps/`, `Emus/` and `Roms/` folders are not used;
   discard them.
4. Put your own matching ROM inside the `<Game>` folder as **rom.z64**.

```text
Games/N64/SuperMario64/
  sm64                 port executable
  launch.sh            port launcher
  rom.z64              your own matching ROM
  data/                created on first launch
```

There is no `App/` folder on the card. MAGI discovers the executable, data
directory and environment from `launch.sh`, so any Miyoo64-style port that
ships one is supported without a per-game entry. The built-in table only covers
legacy loose installs of Super Mario 64, F-Zero X and Majora's Mask.

## In a port

- The first launch builds the port's `data/`; later launches reuse it. Saves
  live there too, and `libn64path` redirects the port's hard-coded
  `App/<Port>/data` paths so they keep working.
- **Volume** adjusts the sound. **SELECT + D-pad** adjusts brightness and
  warmth with the MAGI OSD and saves the choice.
- **POWER** tap sleeps; hold it for the abortable shutdown page. Closing the
  lid sleeps when LID SLEEP is on.
- Bluetooth audio is routed through a small `aplay` bridge, because the 32-bit
  ports cannot load the 64-bit BlueALSA plugin.
- Ports often exit with SIGABRT after writing their clean end marker; MAGI
  treats a non-zero exit as a normal return.
