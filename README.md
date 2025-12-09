# Shady

A tiny Rust desktop app for playing with GLSL tweet-sized fragment shaders locally.

It opens a window with a live GLSL editor on the left and a real-time shader preview on the right. You paste snippets you see on social media and tweak them in-place.

## Features

- Live recompilation of GLSL fragment shader snippets as you type
- Fullscreen triangle rendering via OpenGL (glow) through `eframe` / `egui_glow`
- Small, IDE-like UI: code editor panel + preview panel + status bar
- Built-in example shader (simple radial swirl) with no copyright issues
- Uniforms wired for tweet-style snippets:
  - `FC`   – `vec2`: fragment coordinates relative to the preview rect (pixels)
  - `r`    – `vec2`: preview resolution `(width, height)`
  - `t`    – `float`: time in seconds since app start
  - `o`    – `vec4`: output color you should write in your snippet

## How it works

Internally, Shady wraps your snippet into a complete fragment shader and compiles it on the GPU:

```glsl
#version 330 core
// or #version 300 es on wasm

uniform vec2 r;       // resolution of preview region
uniform float t;      // time in seconds
uniform vec2 rect_min;// top-left corner of preview region in window coords
out vec4 fragColor;

void main() {
    vec2 FC = gl_FragCoord.xy - rect_min; // local coordinates within preview
    vec4 o = vec4(0.0);

    // --- your snippet goes here ---

    fragColor = o;
}
```

The vertex shader renders a fullscreen triangle that covers the preview area.

## Example snippet

The built-in default is a small original radial swirl:

```glsl
// Simple radial swirl
vec2 uv = (FC - r * vec2(0.7, 0.5)) / r.y * 2.0;
float d = length(uv);
float angle = atan(uv.y, uv.x);
float v = 0.5 + 0.5 * sin(8.0 * d - 2.0 * t + 4.0 * angle);
o = vec4(vec3(v), 1.0);
```

You can paste tweet-style snippets that assume variables like `FC`, `r`, `t`, and `o` as long as they assign to `o`.

## Building and running

Prerequisites:

- Rust toolchain (stable) with `cargo`
- A working OpenGL driver

Then from the project root:

```bash
cargo run
```

This will build and launch the app. The window title is `Shady - GLSL tweet shader`.

## Windows DPI manifest

On Windows the app embeds a custom manifest (`shady.manifest`) via `winres` to control DPI awareness:

```xml
<dpiAware>false</dpiAware>
<dpiAwareness>unaware</dpiAwareness>
```

This helps avoid odd scaling/"locking" behavior when moving the window between monitors with different DPI settings.

## Project structure

- `src/main.rs`       – main application (UI, shader pipeline)
- `Cargo.toml`        – Rust crate configuration
- `build.rs`          – build script that embeds the Windows manifest with `winres`
- `shady.manifest`    – Windows application manifest (DPI settings)
- `shady.rc`          – resource script pointing at the manifest
- `.gitignore`        – ignores `target/` and common local/tooling files

## License / reuse

This repository is intended as a small personal tool / reference app. The default shader snippet in `DEFAULT_SNIPPET` is original; you are free to replace it with your own.
