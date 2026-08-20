#!/usr/bin/env python3
"""Validate the approved Corral icon outputs and their integrations.

The default check is read-only and does not build or install anything. It
pins the approved bytes and the complete active integration source files,
then checks image structure, parsed project metadata, shell syntax, and the
release binary's embedded icon bytes.

Use ``--require-build`` after a release build to prove that the 256px PNG is
present in the produced ``corrald-ui`` executable. ``--self-test`` runs the
same checks against temporary fixtures with deliberate corruptions.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import warnings
from collections import Counter
from pathlib import Path
from typing import Callable

from PIL import Image, ImageChops, ImageDraw


ROOT = Path(__file__).resolve().parents[2]

APPROVED_SHA256 = {
    "assets/icon/corral-master.png": "f2274158afa6bda99b9e2a64a140096f5c55aa4daae7dabe89132f8afe385873",
    "assets/icon/corral-icon-1024.png": "e2c754cf3dd7cbc8f10090597360eb56c56fb4672856d48407453dc8190e15e7",
    "assets/icon/corral-icon-256.png": "b3b59cb2c51564ac7aa8d1fe6ffcde0897d83676ed585eedd4284346ca7ae58a",
    "assets/icon/corral-icon-macos.png": "d63c1aa4568deeadf046bf129e0550f396bc639d92f611b340603a28cd080b73",
    "assets/icon/social-preview.png": "9d8ec825b05cb8655fe9aef6d73e61e7ff443b54854b3d502078e3a01d4103ec",
    "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/AppIcon-512@2x.png": "e2c754cf3dd7cbc8f10090597360eb56c56fb4672856d48407453dc8190e15e7",
}

INTEGRATION_SHA256 = {
    "clients/egui/src/main.rs": "09e3162acf7a21fe76efbd869a8b2550ab52fd942b461c4fd7d52a9934202287",
    "tools/icon/from-user-png.py": "a933a13a1da7f8e51a9427a38cd612ec8e5d6eb444cef6f6c378a6287c6e47d9",
    "tools/icon/check-desktop-entry.py": "801960b07623033a96bf22360800be9806ef500c2a9e0f7253957dd03869ba54",
    "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/Contents.json": "5c09bec6eede599b14fa9e4c44b03e7febebc930615a0cd70f02981c09dfe48a",
    "ios/FleetNotifier.xcodeproj/project.pbxproj": "93241932c3cbd975eaef3efed58e68f09bcfd5e19d496e1471bf8683f453368d",
    "scripts/install-corral-ui.sh": "5c6baa4ee6cd8cf95d68efd2fab37a624c23cc5b6199c4ed3c31e81ca809628f",
    "scripts/setup-corrald.sh": "6612c55a1174bbfbc673234ce24441642185a8a24478b1e3eb856a370c3dd33e",
    "scripts/test-icon-packaging.sh": "7e217d13a0a6bdb65206502dc8d7a2c9cbe68c5c158acf6b36e8f7bb9def7644",
}

PNG_SPECS = {
    "assets/icon/corral-master.png": ((1001, 1001), "RGB"),
    "assets/icon/corral-icon-1024.png": ((1024, 1024), "RGB"),
    "assets/icon/corral-icon-256.png": ((256, 256), "RGB"),
    "assets/icon/social-preview.png": ((1280, 640), "RGB"),
    "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/AppIcon-512@2x.png": (
        (1024, 1024),
        "RGB",
    ),
    "assets/icon/corral-icon-macos.png": ((1024, 1024), "RGBA"),
}

MAC_ALPHA_HISTOGRAM = {
    0: 44_432,
    16: 120,
    32: 118,
    48: 110,
    64: 82,
    80: 38,
    96: 84,
    112: 64,
    128: 108,
    144: 44,
    160: 88,
    175: 34,
    176: 26,
    191: 134,
    207: 88,
    223: 74,
    239: 156,
    255: 1_002_776,
}

SOCIAL_BACKGROUND = (1, 1, 1)
SOCIAL_WORDMARK_STATS = (2_775, (423, 284, 622, 330), ((245, 245, 245), 2_122))
SOCIAL_CAPTION_STATS = (3_526, (425, 345, 837, 382), ((170, 170, 170), 1_542))

FIXTURE_FILES = [
    *APPROVED_SHA256,
    "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/Contents.json",
    "ios/FleetNotifier/Info.plist",
    "ios/FleetNotifier.xcodeproj/project.pbxproj",
    "clients/egui/src/main.rs",
    "scripts/setup-corrald.sh",
    "scripts/install-corral-ui.sh",
    "scripts/test-icon-packaging.sh",
    "tools/icon/check-desktop-entry.py",
    "tools/icon/from-user-png.py",
]


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"icon check failed: {message}")


def compatible_pixel_data(image: Image.Image) -> list[int | tuple[int, ...]]:
    """Read pixels through the Pillow API supported by the repository floor."""

    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        return list(image.getdata())


def read_text(root: Path, relative: str) -> str:
    path = root / relative
    require(path.is_file(), f"missing {relative}")
    return path.read_text(encoding="utf-8")


def load_png(root: Path, relative: str) -> Image.Image:
    path = root / relative
    require(path.is_file(), f"missing {relative}")
    expected_size, expected_mode = PNG_SPECS[relative]
    try:
        with Image.open(path) as source:
            image = source.copy()
    except Exception as error:
        raise SystemExit(f"icon check failed: cannot read {relative}: {error}") from error
    require(
        image.size == expected_size,
        f"{relative} has size {image.size}, expected {expected_size}",
    )
    require(
        image.mode == expected_mode,
        f"{relative} has mode {image.mode}, expected {expected_mode}",
    )
    return image


def check_manifest(root: Path, manifest: dict[str, str], label: str) -> None:
    for relative, expected in manifest.items():
        path = root / relative
        require(path.is_file(), f"missing {relative}")
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        require(actual == expected, f"{relative} does not match the approved {label} SHA-256")


def check_hashes(root: Path) -> None:
    check_manifest(root, APPROVED_SHA256, "asset")


def check_integration_hashes(root: Path) -> None:
    check_manifest(root, INTEGRATION_SHA256, "integration source")


def region_stats(
    image: Image.Image, box: tuple[int, int, int, int]
) -> tuple[
    int,
    tuple[int, int, int, int] | None,
    tuple[tuple[int, int, int], int] | None,
]:
    left, top, right, bottom = box
    colors: Counter[tuple[int, int, int]] = Counter()
    points: list[tuple[int, int]] = []
    for y in range(top, bottom):
        for x in range(left, right):
            color = image.getpixel((x, y))
            if color != SOCIAL_BACKGROUND:
                colors[color] += 1
                points.append((x, y))
    if not points:
        return 0, None, None
    xs, ys = zip(*points)
    most_common = colors.most_common(1)[0]
    return len(points), (min(xs), min(ys), max(xs) + 1, max(ys) + 1), most_common


def check_pixels(root: Path) -> None:
    master = load_png(root, "assets/icon/corral-master.png")
    icon_1024 = load_png(root, "assets/icon/corral-icon-1024.png")
    icon_256 = load_png(root, "assets/icon/corral-icon-256.png")
    social = load_png(root, "assets/icon/social-preview.png")
    ios = load_png(
        root,
        "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/AppIcon-512@2x.png",
    )
    mac = load_png(root, "assets/icon/corral-icon-macos.png")

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

    require(
        ImageChops.difference(mac.convert("RGB"), expected_1024).getbbox() is None,
        "macOS output RGB does not match the 1024 output",
    )
    alpha_histogram = Counter(compatible_pixel_data(mac.getchannel("A")))
    require(
        dict(alpha_histogram) == MAC_ALPHA_HISTOGRAM,
        "macOS alpha mask differs from the approved complete mask",
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
    require(
        region_stats(social, (400, 250, 1000, 330)) == SOCIAL_WORDMARK_STATS,
        "social preview wordmark pixels or placement differ from the approved copy",
    )
    require(
        region_stats(social, (400, 345, 1000, 410)) == SOCIAL_CAPTION_STATS,
        "social preview caption pixels or placement differ from the approved copy",
    )


def check_pillow_compatibility() -> None:
    """Exercise the pixel API available on the repository's Pillow floor."""

    image = Image.new("L", (2, 2), 7)
    require(compatible_pixel_data(image) == [7, 7, 7, 7], "Pillow pixel iteration API is unavailable")


def run_bash_syntax(root: Path, relative: str) -> None:
    path = root / relative
    require(path.is_file(), f"missing {relative}")
    result = subprocess.run(
        ["bash", "-n", str(path)], capture_output=True, text=True, check=False
    )
    require(result.returncode == 0, f"{relative} fails bash -n: {result.stderr.strip()}")


def run_python_syntax(root: Path, relative: str) -> None:
    path = root / relative
    require(path.is_file(), f"missing {relative}")
    try:
        compile(path.read_text(encoding="utf-8"), str(path), "exec")
    except (OSError, SyntaxError) as error:
        raise SystemExit(f"icon check failed: {relative} has invalid Python: {error}") from error


def check_references(root: Path) -> None:
    contents_path = root / "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/Contents.json"
    try:
        contents = json.loads(contents_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"icon check failed: invalid iOS AppIcon catalog: {error}") from error
    images = contents.get("images", [])
    expected_image = {
        "filename": "AppIcon-512@2x.png",
        "idiom": "universal",
        "platform": "ios",
        "size": "1024x1024",
    }
    require(expected_image in images, "iOS AppIcon catalog does not reference the 1024 asset")
    require(
        (contents_path.parent / "AppIcon-512@2x.png").is_file(),
        "iOS AppIcon catalog points at a missing PNG",
    )

    try:
        with (root / "ios/FleetNotifier/Info.plist").open("rb") as stream:
            plistlib.load(stream)
    except (OSError, plistlib.InvalidFileException) as error:
        raise SystemExit(f"icon check failed: invalid iOS Info.plist: {error}") from error

    project = read_text(root, "ios/FleetNotifier.xcodeproj/project.pbxproj")
    resources_section = re.search(
        r"/\* Begin PBXResourcesBuildPhase section \*/(?P<section>.*?)/\* End PBXResourcesBuildPhase section \*/",
        project,
        re.DOTALL,
    )
    require(resources_section is not None, "Xcode project has no resources build phase")
    asset_build_file = re.search(
        r"(?P<build_id>[A-F0-9]+) /\* Assets\.xcassets in Resources \*/ = \{"
        r"isa = PBXBuildFile; fileRef = (?P<file_ref>[A-F0-9]+) /\* Assets\.xcassets \*/; \};",
        project,
    )
    require(asset_build_file is not None, "Xcode project has no Assets.xcassets build file")
    build_id = asset_build_file.group("build_id")
    file_ref = asset_build_file.group("file_ref")
    require(
        re.search(
            rf"^\s*{re.escape(build_id)} /\* Assets\.xcassets in Resources \*/,?\s*$",
            resources_section.group("section"),
            re.MULTILINE,
        )
        is not None,
        "Xcode project omits Assets.xcassets from the actual resources phase",
    )
    require(
        re.search(
            rf"{re.escape(file_ref)} /\* Assets\.xcassets \*/ = \{{"
            r"isa = PBXFileReference;[^}]*path = Assets\.xcassets;",
            project,
        )
        is not None,
        "Xcode project has no Assets.xcassets file reference",
    )
    require(
        re.search(r"ASSETCATALOG_COMPILER_APPICON_NAME\s*=\s*AppIcon;", project),
        "Xcode project does not select AppIcon",
    )

    run_bash_syntax(root, "scripts/setup-corrald.sh")
    run_bash_syntax(root, "scripts/install-corral-ui.sh")
    run_bash_syntax(root, "scripts/test-icon-packaging.sh")
    run_python_syntax(root, "tools/icon/from-user-png.py")
    run_python_syntax(root, "tools/icon/check-desktop-entry.py")


def check_build_embedding(root: Path) -> None:
    binary = root / "target/release/corrald-ui"
    require(binary.is_file(), "release corrald-ui binary is missing; run cargo build --release")
    icon = (root / "assets/icon/corral-icon-256.png").read_bytes()
    executable = binary.read_bytes()
    require(icon in executable, "release corrald-ui does not contain the approved 256 icon bytes")


def check_all(root: Path, require_build: bool = False) -> None:
    check_hashes(root)
    check_integration_hashes(root)
    check_pillow_compatibility()
    check_pixels(root)
    check_references(root)
    if require_build:
        check_build_embedding(root)


def make_fixture(destination: Path) -> Path:
    for relative in FIXTURE_FILES:
        source = ROOT / relative
        target = destination / relative
        require(source.is_file(), f"self-test source fixture is missing {relative}")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
    return destination


def expect_rejection(label: str, mutate: Callable[[Path], None]) -> None:
    with tempfile.TemporaryDirectory(prefix="corral-icon-check-") as temporary:
        fixture = make_fixture(Path(temporary))
        check_all(fixture)
        mutate(fixture)
        try:
            check_all(fixture)
        except SystemExit:
            return
        raise AssertionError(f"self-test mutation was accepted: {label}")


def mutate_bytes(relative: str) -> Callable[[Path], None]:
    def mutate(root: Path) -> None:
        path = root / relative
        data = bytearray(path.read_bytes())
        data[len(data) // 2] ^= 1
        path.write_bytes(data)

    return mutate


def mutate_mac_alpha(root: Path) -> None:
    path = root / "assets/icon/corral-icon-macos.png"
    with Image.open(path) as source:
        image = source.copy()
    red, green, blue, _ = image.getpixel((512, 512))
    image.putpixel((512, 512), (red, green, blue, 0))
    image.save(path)


def mutate_social_wordmark(root: Path) -> None:
    path = root / "assets/icon/social-preview.png"
    with Image.open(path) as source:
        image = source.copy()
    ImageDraw.Draw(image).rectangle((450, 285, 500, 315), fill=SOCIAL_BACKGROUND)
    image.save(path)


def mutate_social_caption(root: Path) -> None:
    path = root / "assets/icon/social-preview.png"
    with Image.open(path) as source:
        image = source.copy()
    ImageDraw.Draw(image).rectangle((450, 350, 500, 375), fill=SOCIAL_BACKGROUND)
    image.save(path)


def mutate_appicon_catalog(root: Path) -> None:
    path = root / "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/Contents.json"
    contents = json.loads(path.read_text(encoding="utf-8"))
    contents["images"][0]["filename"] = "wrong-icon.png"
    path.write_text(json.dumps(contents), encoding="utf-8")


def mutate_egui_include(root: Path) -> None:
    path = root / "clients/egui/src/main.rs"
    source = path.read_text(encoding="utf-8")
    path.write_text(source.replace("corral-icon-256.png", "missing-icon.png"), encoding="utf-8")


def mutate_egui_icon_application(root: Path) -> None:
    path = root / "clients/egui/src/main.rs"
    source = path.read_text(encoding="utf-8")
    path.write_text(
        source.replace(".with_icon(app_icon())", "// .with_icon(app_icon())"),
        encoding="utf-8",
    )


def mutate_egui_detached_icon(root: Path) -> None:
    path = root / "clients/egui/src/main.rs"
    source = path.read_text(encoding="utf-8")
    path.write_text(
        source.replace("viewport: viewport_builder(),", "viewport: egui::ViewportBuilder::default(),", 1),
        encoding="utf-8",
    )


def mutate_generator_noop(root: Path) -> None:
    path = root / "tools/icon/from-user-png.py"
    source = path.read_text(encoding="utf-8")
    path.write_text(
        source.replace("def main() -> None:\n", "def main() -> None:\n    return\n", 1),
        encoding="utf-8",
    )


def mutate_appicon_resources_phase(root: Path) -> None:
    path = root / "ios/FleetNotifier.xcodeproj/project.pbxproj"
    source = path.read_text(encoding="utf-8")
    resource_line = "\t\t\t\t21CE07E2350DAFAF99DDC395 /* Assets.xcassets in Resources */,\n"
    require(resource_line in source, "self-test fixture could not find the AppIcon resource phase entry")
    path.write_text(source.replace(resource_line, "", 1), encoding="utf-8")


def mutate_packager(root: Path) -> None:
    path = root / "scripts/install-corral-ui.sh"
    source = path.read_text(encoding="utf-8")
    path.write_text(source.replace("iconutil -c icns", "iconutil -c invalid-mode"), encoding="utf-8")


def mutate_generator_mask(root: Path) -> None:
    path = root / "tools/icon/from-user-png.py"
    source = path.read_text(encoding="utf-8")
    path.write_text(source.replace("Image.BOX", "Image.NEAREST"), encoding="utf-8")


def mutate_generator_fallback(root: Path) -> None:
    path = root / "tools/icon/from-user-png.py"
    source = path.read_text(encoding="utf-8")
    path.write_text(
        source.replace("ImageFont.truetype", "ImageFont.load_default"),
        encoding="utf-8",
    )


def mutate_unembedded_build(root: Path) -> None:
    binary = root / "target/release/corrald-ui"
    binary.parent.mkdir(parents=True, exist_ok=True)
    binary.write_bytes(b"a release-shaped executable without the icon")


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="corral-icon-baseline-") as temporary:
        fixture = make_fixture(Path(temporary))
        check_all(fixture)

    mutations: list[tuple[str, Callable[[Path], None]]] = [
        (f"approved bytes: {relative}", mutate_bytes(relative))
        for relative in APPROVED_SHA256
    ]
    mutations.extend(
        [
            ("macOS alpha hole", mutate_mac_alpha),
            ("social wordmark corruption", mutate_social_wordmark),
            ("social caption corruption", mutate_social_caption),
            ("iOS AppIcon catalog filename", mutate_appicon_catalog),
            ("missing egui include", mutate_egui_include),
            ("commented-out egui icon application", mutate_egui_icon_application),
            ("detached egui icon application", mutate_egui_detached_icon),
            ("immediate-return generator", mutate_generator_noop),
            ("AppIcon removed from Resources phase", mutate_appicon_resources_phase),
            ("packager icon conversion", mutate_packager),
            ("generator mask resampling", mutate_generator_mask),
            ("generator font fallback", mutate_generator_fallback),
        ]
    )
    for label, mutation in mutations:
        expect_rejection(label, mutation)

    with tempfile.TemporaryDirectory(prefix="corral-icon-build-") as temporary:
        fixture = make_fixture(Path(temporary))
        mutate_unembedded_build(fixture)
        try:
            check_build_embedding(fixture)
        except SystemExit:
            pass
        else:
            raise AssertionError("self-test mutation was accepted: unembedded release binary")

    print(f"icon checker self-tests: ok ({len(mutations) + 1} negative cases)")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--require-build",
        action="store_true",
        help="require target/release/corrald-ui to contain the approved icon bytes",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run negative fixture tests in temporary directories",
    )
    args = parser.parse_args()

    if args.self_test:
        self_test()
    else:
        check_all(ROOT, require_build=args.require_build)
        print("icon assets and integration references: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
