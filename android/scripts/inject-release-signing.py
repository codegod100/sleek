#!/usr/bin/env python3
"""Idempotently add [package.metadata.android.signing.release] to
android/Cargo.toml, pointing at a keystore path given on argv.

cargo-apk's --release build requires this section, and it needs an
absolute path to the keystore — which can't be a static committed value
(the repo checkout location isn't fixed: local dev, Nix's store, buck2's
RBE execroot are all different absolute paths). So this runs at build
time instead, same idea flake.nix's own inline Python block already used
for the Nix build — factored out here as a real script so cargo.bzl's
buck2 genrule (cargo_apk_genrule) can call it too without embedding
Python inside a Starlark string inside a shell command.

Usage: inject-release-signing.py <manifest-path> <keystore-path>
"""
import pathlib
import sys

if len(sys.argv) != 3:
    print(f"usage: {sys.argv[0]} <manifest-path> <keystore-path>", file=sys.stderr)
    sys.exit(1)

manifest_path, keystore = sys.argv[1], sys.argv[2]
path = pathlib.Path(manifest_path)
text = path.read_text()
if "signing.release" in text:
    sys.exit(0)

block = f"""
[package.metadata.android.signing.release]
path = "{keystore}"
keystore_password = "android"
key_alias = "androiddebugkey"
key_password = "android"
"""
marker = "[patch.crates-io]"
if marker in text:
    text = text.replace(marker, block + "\n" + marker, 1)
else:
    text = text + block
path.write_text(text)
