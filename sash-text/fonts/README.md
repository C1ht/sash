<!-- SPDX-License-Identifier: Apache-2.0 -->

# Bundled fonts

`Manrope[wght].ttf`: Manrope, a variable font (weight axis 200 to 800), by Mikhail
Sharanda and The Manrope Project Authors, under the SIL Open Font License 1.1
(`OFL.txt`, kept next to the font). No Reserved Font Name is declared. It is embedded
into sash-text with `include_bytes!` and is never sold on its own.

Source: github.com/google/fonts, `ofl/manrope` at commit
b31870aff700ab7a1d74fa0c6887d95beb9e0037.

| File | sha256 |
|---|---|
| Manrope[wght].ttf | 3ae11c49db0455a3cc33e37d380f20fdb8c7f8b41dc07625c177e3d87a9d6ae6 |
| OFL.txt | 58172e0c0fac2cda8a37b348164bb55e44b0e69051e557e92b1d3f6910141f7b |

The fallback fonts (Noto, DejaVu) are never bundled: they are read from
/usr/share/fonts when installed (see `src/fonts.rs`).
