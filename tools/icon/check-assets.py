#!/usr/bin/env python3
"""Validate checked-in Corral icon outputs and their integration points.

This check is read-only. It intentionally validates the generated pixels and
the references used by egui, Xcode, and the platform packaging script without
building an app or touching an installed client.
"""

from __future__ import annotations

import json
import plistlib
import sys
from pathlib import Path

from PIL import Image, ImageChops


ROOT = Path(__file__).resolve().parents[2]


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"icon check failed: {message}")


def load_png(relative: str, size: tuple[int, int], mode: str) -> Image.Image:
    path = ROOT / relative
    require(path.is_file(), f"missing {relative}")
    image = Image.open(path)
    require(image.size == size, f"{relative} has size {image.size}, expected {size}")
    require(image.mode == mode, f"{relative} has mode {image.mode}, expected {mode}")
    return image.copy()


def check_pixels() -> None:
    master = load_png("assets/icon/corral-master.png", (1001, 1001), "RGB")
    icon_1024 = load_png("assets/icon/corral-icon-1024.png", (1024, 1024), "RGB")
    icon_256 = load_png("assets/icon/corral-icon-256.png", (256, 256), "RGB")
    social = load_png("assets/icon/social-preview.png", (1280, 640), "RGB")
    ios = load_png(
        "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/AppIcon-512@2x.png",
        (1024, 1024),
        "RGB",
    )
    mac = load_png("assets/icon/corral-icon-macos.png", (1024, 1024), "RGBA")

    require(
        icon_1024.tobytes() == ios.tobytes(), "iOS and repository 1024 icons differ"
    )

    expected_1024 = master.resize((1024, 1024), Image.Resampling.LANCZOS)
    expected_256 = master.resize((256, 256), Image.Resampling.LANCZOS)
    require(
        ImageChops.difference(icon_1024, expected_1024).getbbox() is None,
        "1024 output does not match the master resize",
    )
    require(
        ImageChops.difference(icon_256, expected_256).getbbox() is None,
        "256 output does not match the master resize",
    )

    expected_mac = expected_1024
    require(
        ImageChops.difference(mac.convert("RGB"), expected_mac).getbbox() is None,
        "macOS output RGB does not match the 1024 output",
    )
    alpha = mac.getchannel("A")
    require(
        alpha.getextrema() == (0, 255), "macOS output has no transparent/opaque alpha"
    )
    require(
        all(
            mac.getpixel(point)[3] == 0
            for point in ((0, 0), (1023, 0), (0, 1023), (1023, 1023))
        ),
        "macOS output corners are not transparent",
    )
    require(mac.getpixel((512, 512))[3] == 255, "macOS output center is not opaque")

    social_icon = social.crop((70, 170, 370, 470))
    expected_social_icon = master.resize((300, 300), Image.Resampling.LANCZOS)
    require(
        ImageChops.difference(social_icon, expected_social_icon).getbbox() is None,
        "social preview icon does not match the master resize",
    )


def check_references() -> None:
    contents = json.loads(
        (
            ROOT / "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/Contents.json"
        ).read_text()
    )
    images = contents.get("images", [])
    require(
        {
            "filename": "AppIcon-512@2x.png",
            "idiom": "universal",
            "platform": "ios",
            "size": "1024x1024",
        }
        in images,
        "iOS AppIcon catalog does not reference the 1024 asset",
    )

    with (ROOT / "ios/FleetNotifier/Info.plist").open("rb") as stream:
        plistlib.load(stream)

    project = (ROOT / "ios/FleetNotifier.xcodeproj/project.pbxproj").read_text()
    require(
        "Assets.xcassets in Resources" in project, "Xcode project omits Assets.xcassets"
    )
    require(
        "ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon;" in project,
        "Xcode project does not select AppIcon",
    )

    egui = (ROOT / "clients/egui/src/main.rs").read_text()
    require(
        'include_bytes!("../../../assets/icon/corral-icon-256.png")' in egui,
        "egui does not embed the 256 icon",
    )
    require(
        ".with_icon(app_icon())" in egui, "egui viewport does not use the embedded icon"
    )

    setup = (ROOT / "scripts/setup-corrald.sh").read_text()
    for needle in (
        "assets/icon/corral-icon-macos.png",
        "iconutil -c icns",
        "CFBundleIconFile",
        "assets/icon/corral-icon-256.png",
        "Icon=corral",
        "trap 'rm -rf -- \"$icon_tmp\"' EXIT",
    ):
        require(needle in setup, f"setup script missing {needle!r}")

    generator = (ROOT / "tools/icon/from-user-png.py").read_text()
    for needle in (
        "assets/icon/corral-icon-macos.png",
        "rounded_rectangle",
        "Image.BOX",
    ):
        require(needle in generator, f"icon generator missing {needle!r}")


def main() -> int:
    check_pixels()
    check_references()
    print("icon assets and integration references: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
