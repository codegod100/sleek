# Sleek

Mobile **freeq** client built with **[Vidya](https://tangled.org/nandi.uk/vidya)** (GNOME/HIG-inspired egui theme) and **[freeq-sdk](https://github.com/codegod100/freeq/tree/main/freeq-sdk)**.

Layout and flows take cues from the freeq Android app: connect (guest), chats list, chat detail, discover, and settings — with a portrait bottom-tab shell.

## Screenshot

Sleek on Waydroid (chat + in-call video):

<p align="center">
  <img src="docs/waydroid.png" alt="Sleek on Waydroid — chat with live call overlay" width="320" />
</p>

## Screens

| Screen | Role |
|--------|------|
| **Connect** | Nick + server, guest connect (TLS default `irc.freeq.at:6697`) |
| **Chats** | Channel/DM list with last preview, unread badges, search, join |
| **Chat** | Message stream + compose bar; Search finds messages across chats |
| **Discover** | Popular channels + custom join |
| **Settings** | Account, connection status, dark/light shell, disconnect |

## Stack

- **UI**: [egui](https://github.com/emilk/egui) + [vidya](https://tangled.org/nandi.uk/vidya) theme/widgets + Android safe chrome
- **Network**: [freeq-sdk](https://github.com/codegod100/freeq/tree/main/freeq-sdk) (guest IRC, TLS / WebSocket)
- **Targets**: desktop host (Wayland/X11) and Android NativeActivity (`cargo-apk` / Waydroid)

## Installation

Install via Flatpak (hosted OSTree repo on `artifacts.latha.org`):

```bash
flatpak install --user https://artifacts.latha.org/artifacts/sleek/uk.nandi.sleek.flatpakref
flatpak run uk.nandi.sleek
```

## Run

Dev shell + desktop host build work via either **pixi** or **Nix** — pick one.
Android/Waydroid/Flatpak packaging, Buck2, and Cachix pushes below are still
Nix-only (`flake.nix`); `pixi.toml` only replaces `devenv.nix`'s job.

```bash
# pixi (conda-forge toolchain — no /nix/store, works on any Linux box):
pixi install        # once, materializes .pixi/envs/default from pixi.lock
pixi shell           # or: pixi run <task>
just host            # desktop window (cargo run) — same recipe either shell
just lib             # build android package as rlib (desktop target)
```

```bash
just host          # desktop window — buck2 build + run (see BUCK, cargo.bzl)
just lib           # build android package as rlib (desktop target, via buck2)
just waydroid      # cargo-apk → install → launch on Waydroid (x86_64)
nix develop        # or: direnv allow  (after .envrc)

# Or via the flake:
nix run            # flake devShell + just host --release (needs sibling vidya/freeq)
nix run .#host     # same as nix run
nix run .#sleek    # pure Nix store binary (hermetic)
nix build .#sleek  # → ./result/bin/sleek
nix build .#flatpak  # → ./result/uk.nandi.sleek.flatpak (GNOME Platform 49)

# Waydroid (x86_64 cargo-apk + install/launch + full UI window):
nix run .#waydroid                 # debug: build + install + launch + show-full-ui
nix run .#waydroid-release         # release (optimized + local signing keystore)

# Phone APK (aarch64) + adb install:
nix run .#deploy-android              # cargo apk + adb install -r
nix run .#deploy-android -- --launch  # …and start the activity
# Pure Nix store build (reproducible):
just android                   # nix build .#android
nix run .#install-android      # adb install -r that store APK

# Desktop Flatpak bundle:
just flatpak                   # → result-flatpak/*.flatpak
# Install: flatpak install --user ./result-flatpak/*.flatpak
```

Guest connect defaults:

- Server: `irc.freeq.at:6697` (TLS)
- Nick: random `sleekXXXX` (editable)

On Android, the client prefers WebSocket (`wss://host/irc`) when the host looks like a freeq public server; desktop uses TLS TCP by default and can fall back to WebSocket via the connect form.

## Install

**Single-command Flatpak install** (OSTree repo on `artifacts.latha.org`):

```bash
flatpak install --user https://artifacts.latha.org/artifacts/sleek/uk.nandi.sleek.flatpakref
flatpak run uk.nandi.sleek
```

Or install from a tagged `.flatpak` bundle:

```bash
curl -LO https://tangled.org/nandi.uk/sleek/tags/v0.1.5/download/uk.nandi.sleek.flatpak
flatpak install --user ./uk.nandi.sleek.flatpak
flatpak run uk.nandi.sleek
```

Release artifacts are attached to version tags on [tangled.org/nandi.uk/sleek](https://tangled.org/nandi.uk/sleek) — visible under **Artifacts** on each tag's page.

## CI

Merge / PR CI (check + APK + Flatpak) runs on **boxci** — see [`.boxci/pipeline.yml`](.boxci/pipeline.yml).

| Job | Artifact |
|-----|----------|
| APK | `sleek.apk` |
| Flatpak | `uk.nandi.sleek.flatpak` |

CI APKs are signed with `android/ci.keystore` (alias `androiddebugkey`). If you previously installed a build signed with a different key, uninstall Sleek first before installing a new CI build.

## Layout

```
sleek/
  android/          # shared lib: UI + freeq-sdk bridge (cdylib for APK)
  host/             # desktop binary
  assets/           # desktop entry + icons (Flatpak / host)
  .tangled/         # Spindle CI (Tangled)
  scripts/          # enter, codespace shim, flakes ensure, Waydroid
  .github/workflows # CI: APK + Flatpak artifacts
  .devcontainer/    # GitHub Codespaces (nix feature + flakes)
  .envrc            # direnv → flake
  justfile
  flake.nix         # .#sleek, .#android, .#flatpak, …
```

## License

MIT
