# Vendored `egui_glow` 0.31.1 patches

Source: crates.io `egui_glow` 0.31.1 (matches eframe/egui 0.31).

## Why

On Android GLES (and other GL_ES targets), stock `egui_glow` 0.31.1 fragment/vertex
shaders use `precision mediump float;` (fp16). Font-atlas UVs in `[0,1]` then lose
sub-pixel addressability once the atlas grows past ~1024² — glyphs sample neighboring
texels and look like thin, fragmented, or “boxy” characters.

This shows up clearly on Sleek’s colored avatar initials (Users list / chat), where
large single letters sit on high-contrast circles. Same class of bug as
[emilk/egui#4268](https://github.com/emilk/egui/issues/4268).

## Changes

1. **`src/shader/fragment.glsl`** and **`src/shader/vertex.glsl`**: prefer `highp` when
   `GL_FRAGMENT_PRECISION_HIGH` is available (same as upstream
   [emilk/egui#6893](https://github.com/emilk/egui/pull/6893), shipped in egui 0.32).

Remove this vendor once upgrading to egui ≥ 0.32.
