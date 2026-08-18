# Vendored cpal

Pinned from https://github.com/RustAudio/cpal.git at
`63ea9fffbd3b15af0180f0800a61a3c4fec9b88a` (reported as 0.18.0).

## Why

`moq-media` (iroh-live) depends on `cpal` with `branch = "master"`, which
floats. Newer master commits remove `cpal::StreamError` and
`DeviceTrait::name()` (unified `Error` / `description()` API). That breaks
moq-media's `audio_backend.rs` at iroh-live rev `edd9bcc`.

Same commit freeq locks in its workspace `Cargo.lock`.

## Update

When iroh-live/moq-media is updated to the new cpal API, drop this vendor
and the `[patch."https://github.com/RustAudio/cpal.git"]` entries in
`android/Cargo.toml` and `host/Cargo.toml`.
