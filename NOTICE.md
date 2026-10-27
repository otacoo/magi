# NOTICE

MAGI is licensed under the BSD 2-Clause License. See [LICENSE.md](LICENSE.md).

Copyright (c) 2026, otacoo. All rights reserved.

## Bundled third-party assets

- **JetBrains Mono** (`assets/fonts/JetBrainsMono-Regular.ttf`) is licensed
  under the SIL Open Font License 1.1. See `assets/fonts/OFL.txt`. JetBrains
  Mono is a trademark of JetBrains s.r.o.
- **GEHIRN boot art** (`assets/boot/gehirn.png`) is "the human brain" by Crean
  Quaner, published in the Brain Art gallery of The Neuro Bureau (June 8,
  2018): <https://www.neurobureau.org/abstract_the-human-brain_by_crean-quaner/>.
  It is licensed under Creative Commons Attribution-ShareAlike 4.0
  International (CC BY-SA 4.0):
  <https://creativecommons.org/licenses/by-sa/4.0/>. The copy shipped here is
  modified for MAGI's boot sequence (640x480, transparent background,
  alpha-composited); the derivative remains under CC BY-SA 4.0. See
  `assets/boot/SOURCE.md`. The boot and README images that contain this art
  are covered by the same license.
- **Perfect Overlays** (`assets/overlays/`) comes from
  <https://github.com/ourigen/Perfect_Overlays> (640x480 overlays for
  handhelds), derived from the work of 1playerinsertcoin, with bezels by
  mugwomp_93 and drkhrse
  (<https://github.com/drkhrse/drkhrse_miyoo_bezels>). The upstream repository
  carries no license file; it is included with the project owner's approval
  and credited here. See `assets/overlays/SOURCE.md`.
- **SDL Game Controller DB** (`assets/controllers/gamecontrollerdb.txt`) is
  the community controller mapping database
  (<https://github.com/mdqinc/SDL_GameControllerDB>), licensed under the zlib
  license. See `assets/controllers/LICENSE`.
- **SDL 1.2** (`assets/n64/libSDL-1.2.so.0`) is the SDL 1.2.15 library from
  the Miyoo Mini toolchain sysroot
  (<https://github.com/shauninman/miyoomini-toolchain-buildroot>), licensed
  under the zlib license. See `assets/n64/LICENSE-SDL`.

## First-party runtime components

- **`libmiyoofb.so`** (`tools/n64/sdlfb_shim.c`) is MAGI's SDL 1.2 framebuffer
  and audio shim for native N64 ports, licensed under BSD 2-Clause.
- **`libn64path.so`** (`tools/n64/n64path.c`) is MAGI's save-path and sleep
  shim for native N64 ports, licensed under BSD 2-Clause.

## Linked dependencies

- **SDL2** is licensed under the zlib license.
- Rust crates retain their respective licenses. See `Cargo.lock` and each
  project's documentation.

## Separate components

- **libretro cores** are separate components. They are installed on the
  user's card and carry their own licenses; the payload ships their license
  texts under `System/magi/cores/licenses/`.
- **Native N64 ports** are not part of MAGI and are not distributed with it.
  They are built and supplied separately by their respective authors.
- **ROMs, BIOS files, and game data** are not part of MAGI and are not
  distributed with it.

## Downloaded on request

- **Box art** is fetched from
  [libretro-thumbnails](https://github.com/libretro-thumbnails/libretro-thumbnails)
  only when the user asks for it and the card has Wi-Fi. Nothing is bundled;
  each image remains the property of its publisher.

## Trademarks

Anbernic, BaseOS, SDL, JetBrains Mono, and all other product names are
trademarks or registered trademarks of their respective owners. Use of these
names does not imply endorsement.
