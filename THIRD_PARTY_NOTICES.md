# Third-party notices

## Sharp-Shimmerless-Shader

- Author: zadpos
- Source: https://github.com/Woohyun-Kang/Sharp-Shimmerless-Shader
- License: CC0 1.0 Universal (public domain dedication)

The box-filter sample point used by MAGI's `SHIMMERLESS` screen shader in
`src/present/gles.rs` is ported from
`shaders_glsl/shaders/sharp-shimmerless.glsl`.

## libretro/glsl-shaders handheld set

The built-in `GBA COLOR`, `GBC COLOR`, `LCD3X` and `DOT` modes
are ports of public-domain shaders from
https://github.com/libretro/glsl-shaders/tree/master/handheld:

- Color Mangler (`gba-color`, `gbc-color`)
  Author: hunterk; modified by Pokefan531. License: Public domain.
- `lcd3x`
  Author: Gigaherz. License: Public domain.
- `dot`
  Author: Themaister. License: Public domain.

They are ported into the single-pass MAGI screen shader in
`src/present/gles.rs`.
