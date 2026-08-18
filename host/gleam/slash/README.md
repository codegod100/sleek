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
