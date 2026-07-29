# Sleek

Mobile **freeq** client built with **[Vidya](../vidya)** (GNOME/HIG-inspired egui theme) and **[freeq-sdk](../freeq/freeq-sdk)**.

Layout and flows take cues from the freeq Android app: connect (guest), chats list, chat detail, discover, and settings — with a portrait bottom-tab shell.

## Screens

| Screen | Role |
|--------|------|
| **Connect** | Nick + server, guest connect (TLS default `irc.freeq.at:6697`) |
| **Chats** | Channel/DM list with last preview, unread badges, search, join |
| **Chat** | Message stream + compose bar; back to list |
| **Discover** | Popular channels + custom join |
| **Settings** | Account, connection status, dark/light shell, disconnect |

## Stack

- **UI**: [egui](https://github.com/emilk/egui) + [vidya](../vidya) theme/widgets + Android safe chrome
- **Network**: [freeq-sdk](../freeq/freeq-sdk) (guest IRC, TLS / WebSocket)
- **Targets**: desktop host (Wayland/X11) and Android NativeActivity (`cargo-apk` / Waydroid)

## Run

```bash
nix develop        # or: direnv allow  (after .envrc)
# or: ./scripts/enter
just host          # desktop window (cargo run)
just lib           # build android package as rlib (desktop target)
just waydroid      # APK → install → launch on Waydroid

# Or via the flake (builds desktop host into ./result):
nix build          # → ./result/bin/sleek
nix run            # build + run desktop host
```

The dev shell does **not** set ambient `LD_LIBRARY_PATH` (that broke Ubuntu
`git pull` on Codespaces via nix openssl/glibc). Runtime libs for the desktop
host live in `SLEEK_LD_LIBRARY_PATH` and are applied by `just host` only.

If you still see `GLIBC_ABI_DT_X86_64_PLT` on an old session:

```bash
unset LD_LIBRARY_PATH
./scripts/enter          # re-enter flake shell (nix git/curl on PATH)
git pull
```

Guest connect defaults:

- Server: `irc.freeq.at:6697` (TLS)
- Nick: random `sleekXXXX` (editable)

On Android, the client prefers WebSocket (`wss://host/irc`) when the host looks like a freeq public server; desktop uses TLS TCP by default and can fall back to WebSocket via the connect form.

## Codespaces / `gh codespace ssh`

Codespaces uses a **nix-codespace** setup: Ubuntu 24.04 base, Nix installed
by bootstrap (not the official Nix *feature* — that feature’s `/nix` volume
mount often fails Codespace create and drops you into Alpine recovery), with
**flakes and `nix-command` always enabled** (no
`--extra-experimental-features` flags).

| Piece | Role |
|-------|------|
| `.devcontainer/devcontainer.json` | Ubuntu base + `NIX_CONFIG` + postCreate bootstrap |
| `scripts/ensure-nix-flakes.sh` | writes user/system `nix.conf` + `NIX_CONFIG` so flakes stay on |
| `.envrc` | direnv `use flake` |
| `scripts/enter` | manual / scripted re-exec into `nix develop` |
| `scripts/codespace-env.sh` | sourced from `~/.bashrc` on interactive login |
| `scripts/codespace-bootstrap.sh` | installs nix (Determinate), flakes config, direnv, bashrc hook, warms flake |

Bare commands work after bootstrap:

```bash
nix develop          # no flags
nix build
nix run
just host
```

```bash
# Create / open a codespace on this repo, then:
gh codespace ssh
# → bashrc sources codespace-env.sh → nix develop (rustc, just, …)

# Opt out for one session:
SLEEK_NO_AUTO_NIX=1 gh codespace ssh

# Force enter without login hook:
./scripts/enter
./scripts/enter just host
```

First create installs the Nix feature into the container, then runs
`codespace-bootstrap.sh` (flake warm can take a few minutes). Later SSH
sessions re-enter the shell only.

## Layout

```
sleek/
  android/          # shared lib: UI + freeq-sdk bridge (cdylib for APK)
  host/             # desktop binary
  scripts/          # enter, codespace shim, flakes ensure, Waydroid
  .devcontainer/    # GitHub Codespaces (nix feature + flakes)
  .envrc            # direnv → flake
  justfile
  flake.nix
```

## License

MIT
