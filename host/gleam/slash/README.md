# gleam_slash

Compose-box slash parser as a Gleam → Wasm guest for the sleek desktop host.

## Layout

- `parse(input) -> Slash` — custom type decoded by `host/src/gleam_slash.rs`
- String ops (`length`, `byte_at`, `slice`, `lowercase`) are `@external(wasm, "sleek", …)`
  provided by the wasmtime linker (no Gleam stdlib on the Wasm target yet)

## Smoke

```bash
nix develop --command just gleam-slash
# or: cargo run --manifest-path host/Cargo.toml -- --gleam-slash-only
```

Compares Gleam output to `sleek::slash::parse_native` for the same cases.

## Prebuilt Wasm

`host/build.rs` falls back to `prebuilt/gleam_slash.wasm` when a wasm-capable
`gleam` binary is not available (hermetic `nix build .#sleek` / Flatpak CI).
Regenerate after editing `src/gleam_slash.gleam`:

```bash
export GLEAM=/path/to/nandi.uk/gleam/target/release/gleam  # branch wasm
(cd host/gleam/slash && "$GLEAM" build)
cp host/gleam/slash/build/dev/wasm/gleam_slash/gleam_slash.wasm \
  host/gleam/slash/prebuilt/gleam_slash.wasm
```
