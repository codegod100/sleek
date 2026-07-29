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

If a **system** tool (Ubuntu `git`, etc.) dies with `GLIBC_ABI_DT_X86_64_PLT` inside
`nix develop`, either re-enter the shell (flake ships nix `git`/`curl`) or:

```bash
env -u LD_LIBRARY_PATH git pull
```

Guest connect defaults:

- Server: `irc.freeq.at:6697` (TLS)
- Nick: random `sleekXXXX` (editable)

On Android, the client prefers WebSocket (`wss://host/irc`) when the host looks like a freeq public server; desktop uses TLS TCP by default and can fall back to WebSocket via the connect form.

## Codespaces / `gh codespace ssh`

The repo ships a small nix **shim** so an SSH session lands in the flake shell:

| Piece | Role |
|-------|------|
| `.envrc` | direnv `use flake` |
| `scripts/enter` | manual / scripted re-exec into `nix develop` |
| `scripts/codespace-env.sh` | sourced from `~/.bashrc` on interactive login |
| `scripts/codespace-bootstrap.sh` | installs nix + direnv, hooks bashrc, warms flake |
| `.devcontainer/devcontainer.json` | Codespace image + `postCreate` / `postStart` bootstrap |

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

First create runs `codespace-bootstrap.sh` (nix install can take a few minutes). Later SSH sessions reuse the profile and only re-enter the shell.

## Layout

```
sleek/
  android/          # shared lib: UI + freeq-sdk bridge (cdylib for APK)
  host/             # desktop binary
  scripts/          # enter, codespace shim, Waydroid
  .devcontainer/    # GitHub Codespaces
  .envrc            # direnv → flake
  justfile
  flake.nix
```

## License

MIT
