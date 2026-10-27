# N64 native ports runtime

The Miyoo64 recompilations are 32-bit ARM builds that use SDL 1.2, a
software renderer and the Miyoo keypad. BaseOS runs the binaries (32-bit
compat) but has no SDL 1.2 and no Miyoo panel/audioserver.

- `libSDL-1.2.so.0` is the SDL 1.2.15 sysroot library from
  [shauninman/miyoomini-toolchain-buildroot](https://github.com/shauninman/miyoomini-toolchain-buildroot)
  v0.0.3, the same build the ports link against (fbcon video, evdev input,
  OSS audio). zlib license, see `LICENSE-SDL`.
- `libmiyoofb.so` is MAGI's armhf `LD_PRELOAD` shim (source:
  `tools/n64/sdlfb_shim.c`). It turns SDL's video into a software surface
  copied to `/dev/fb0`, feeds the port from the H700 pad over evdev, and
  plays the port's SDL audio callback through ALSA. It also applies MAGI's
  volume/temperature and draws the supervisor's OSD over the frame.
- `libn64path.so` (source: `tools/n64/n64path.c`) redirects the ports'
  hard-coded `App/<Port>/data` paths to the game folder's `data` directory,
  so save states work with the `Games/N64/<Game>` layout.

No game data or ROMs are shipped. Install a Miyoo64 port as
`Games/N64/<Game>/`: copy the port's `App/<Game>/` contents there and put
your own matching ROM beside them as `rom.z64`. Only that App subfolder is
needed; the Miyoo launcher's `Apps/`, `Emus/` and `Roms/` folders are not
used. MAGI reads the executable and `*_DATA` path from the port's
`launch.sh`, so new ports need no launcher table entry. On first launch the
game builds its data in that folder.
