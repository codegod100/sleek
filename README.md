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
nix develop
just host          # desktop window
just lib           # build android package as rlib (desktop target)
just waydroid      # APK → install → launch on Waydroid
```

Guest connect defaults:

- Server: `irc.freeq.at:6697` (TLS)
- Nick: random `sleekXXXX` (editable)

On Android, the client prefers WebSocket (`wss://host/irc`) when the host looks like a freeq public server; desktop uses TLS TCP by default and can fall back to WebSocket via the connect form.

## Layout

```
sleek/
  android/     # shared lib: UI + freeq-sdk bridge (cdylib for APK)
  host/        # desktop binary
  scripts/     # Waydroid install/launch
  justfile
  flake.nix
```

## License

MIT
