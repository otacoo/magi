// MAGI build glue: SDL2's static Windows build needs advapi32 (registry
// lookups in SDL_windowsaudio/mouse) which sdl2-sys 0.37 does not link.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=Advapi32");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
