#!/usr/bin/env python3
"""Build Sleek's Relm4 APK with GTK Android Builder."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
from pathlib import Path


def run(*args: str) -> None:
    subprocess.run(args, check=True)


def find_android_home(explicit: str | None) -> Path:
    candidates = [
        explicit,
        os.environ.get("ANDROID_HOME"),
        os.environ.get("ANDROID_SDK_ROOT"),
        str(Path.home() / ".local/share/android-sdk"),
        str(Path.home() / "Android/Sdk"),
    ]
    for candidate in candidates:
        if candidate and (Path(candidate).expanduser() / "platform-tools/adb").is_file():
            return Path(candidate).expanduser().resolve()
    raise SystemExit("Android SDK not found; set ANDROID_HOME")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--android-home")
    parser.add_argument("--output", type=Path, default=Path("sleek-relm4.apk"))
    args = parser.parse_args()

    root = Path(__file__).resolve().parent.parent
    sdk = find_android_home(args.android_home)
    pixiewood = shutil.which("pixiewood")
    if not pixiewood:
        raise SystemExit("pixiewood is not on PATH; enter the Sleek Nix shell")

    run(pixiewood, "-C", str(root), "prepare", "-s", str(sdk), "pixiewood.xml")
    run(pixiewood, "-C", str(root), "generate")

    manifest = root / ".pixiewood/android/app/src/main/AndroidManifest.xml"
    text = manifest.read_text()
    permission = '  <uses-permission android:name="android.permission.INTERNET"/>\n'
    if "android.permission.INTERNET" not in text:
        marker = '  <uses-permission android:name="android.permission.REORDER_TASKS"/>'
        if marker not in text:
            raise RuntimeError("Pixiewood manifest permission marker is missing")
        manifest.write_text(text.replace(marker, permission + marker))

    run(pixiewood, "-C", str(root), "build")
    output_root = root / ".pixiewood/android/app/build/outputs/apk"
    apks = sorted(output_root.glob("**/*.apk"))
    if len(apks) != 1:
        raise RuntimeError(f"expected one APK under {output_root}, found {len(apks)}")
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(apks[0], output)
    print(output)


if __name__ == "__main__":
    main()
