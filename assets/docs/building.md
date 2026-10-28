# Building

Desktop development uses the SDL2 harness.

```sh
cargo run                 # 1x window, payload-latest/ as the card
cargo run -- --selftest   # headless frames in target/selftest/
cargo fmt
cargo test
```

The device build is CI-only: `.github/workflows/build.yml` cross-compiles in a Debian bullseye container, keeps the glibc symbol floor at 2.31, builds the pinned cores and uploads the 640x480 and 720x480 payloads.

```sh
tools/gehirn/package.sh target/aarch64-unknown-linux-gnu/release/magi dist
tools/gehirn/deploy.sh adb
tools/gehirn/deploy.sh root@rg35xxsp.local
```

On Windows it requires Docker to build the payload; `release.cmd` wraps the host build, tests, CI payload download, adb deploy, ROM transfer, log and screenshot pulls, and a shell on the device. If a `payload-latest/` folder is present `release.cmd` can `cargo run` to use it as the card: its `systems.json`, `Games/`, `Covers/`, `Saves/` and `Config/` are what the shelf reads and writes. Set `MAGI_CARD=/path/to/card` to use another folder, or `MAGI_SCALE=2` to double the window.

## Instrumentality Layout

| Path | Role |
|---|---|
| `src/main.rs` | Multicall entry point: `magi` is the shelf, `magi run` is the game host. |
| `src/app.rs` | Shelf state and event loop. |
| `src/screens/` | Library, settings, network, power, controls and boot screens. |
| `src/run.rs` | libretro host loop, core plumbing, save states and the OSD. |
| `src/run/menu*` | Pause-menu state, navigation and pages. |
| `src/run/overlay.rs` | Overlay scan, composite, blend and per-system choices. |
| `src/run/pad.rs` | SDL pad events translated to the vendor event pipeline. |
| `src/run/prefs.rs` | Runtime JSON store, display prefs and scale modes. |
| `src/n64.rs` | Native port discovery and supervision. |
| `src/present/` | EGL/GLES, direct framebuffer, DE layer and SDL presenters. |
| `src/theme.rs` | Every colour and type scale in the UI. |
| `tools/gehirn/` | Core pins, card packaging and deploy scripts. |
| `tools/n64/` | SDL framebuffer/audio and save-path shims. |
| `assets/` | Systems, boot art, font, overlays, controller DB, docs and readme images. |

Each process writes its own capped log in `Logs/` (`magi.log`, `magi-run.log`, the N64 logs). `magi --probe` prints an environment report for device debugging.