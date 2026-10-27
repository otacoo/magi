# SDL Game Controller DB

`gamecontrollerdb.txt` comes from the community database at
https://github.com/mdqinc/SDL_GameControllerDB (formerly gabomdq).

It extends SDL's built-in mappings, so Bluetooth and USB pads that SDL can
open as game controllers (Xbox, DualSense, Switch, generic HID) get the
community's corrections. MAGI loads this file explicitly at startup
(`System/magi/controllers/gamecontrollerdb.txt`); it is not required — SDL's
built-in database still applies without it.

Licensed under the zlib license, same as SDL; see `LICENSE` in this folder.
