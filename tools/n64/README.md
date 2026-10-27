# N64 shim build

`assets/n64/libmiyoofb.so` is built from `sdlfb_shim.c` with Zig's armhf
target against the Miyoo toolchain's SDL 1.2 sysroot:

```
zig cc -target arm-linux-gnueabihf -shared -fPIC \
  -I<miyoomini-toolchain>/arm-linux-gnueabihf/libc/usr/include \
  sdlfb_shim.c <same sysroot>/lib/libSDL-1.2.so.0.11.4 \
  -Wl,-z,lazy -lpthread -o libmiyoofb.so
```

The shim loads `libasound.so.2` with `dlopen` (the 32-bit ALSA library is
part of BaseOS) and has no other dependencies beyond libc and SDL.
