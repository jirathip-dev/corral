#!/usr/bin/env python3
"""Validate the approved Corral icon outputs and their integrations.

The default check is read-only and does not build or install anything. It
pins the approved bytes and the complete active integration source files,
then checks image structure, parsed project metadata, and shell/Python
syntax.

``--self-test`` runs the same checks against temporary fixtures with
deliberate corruptions.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
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
from types import ModuleType
from typing import Callable

from PIL import Image, ImageChops, ImageDraw


ROOT = Path(__file__).resolve().parents[2]

APPROVED_SHA256 = {
    "assets/icon/corral-master.png": "f2274158afa6bda99b9e2a64a140096f5c55aa4daae7dabe89132f8afe385873",
    "assets/icon/corral-icon-1024.png": "e2c754cf3dd7cbc8f10090597360eb56c56fb4672856d48407453dc8190e15e7",
    "assets/icon/corral-icon-256.png": "b3b59cb2c51564ac7aa8d1fe6ffcde0897d83676ed585eedd4284346ca7ae58a",
    "assets/icon/corral-icon-macos.png": "8c20a7a96f7405e51d3e49dcdb6477720645dc2c46116c9e034313b409de09d6",
    "assets/icon/social-preview.png": "9d8ec825b05cb8655fe9aef6d73e61e7ff443b54854b3d502078e3a01d4103ec",
    # #463: the shipping app icon is the approved Treatment-A Bay master;
    # the legacy Original bytes above stay only as historical repository art.
    "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png": "e9e8e7ebb922660dd76d511dd039562dbb333edc59080d8ffdb9efe19c1590c9",
    "ios/FleetNotifier/Assets.xcassets/Palomino.appiconset/Palomino-1024.png": "7e04da3407296c11f94f929124d4b6c82d83bce70e3a2700c6af906774d863ec",
    "ios/FleetNotifier/Assets.xcassets/Black.appiconset/Black-1024.png": "3ff9ccc5d592a3f43c59f8fbdf09d95a75d8200666e6fc04a836babdee77e32c",
    "ios/FleetNotifier/Assets.xcassets/Grey.appiconset/Grey-1024.png": "2d43e4c3a360ddc8d8fade528f227b8808b2bb2baaef0bb816a0373cb738402e",
    # #464: the four loadable preview imagesets carry the SAME approved
    # Treatment-A master bytes verbatim (iOS 18+ does not vend appiconset
    # renditions to UIImage, so the Settings picker previews need imagesets).
    "ios/FleetNotifier/Assets.xcassets/BayPreview.imageset/BayPreview-1024.png": "e9e8e7ebb922660dd76d511dd039562dbb333edc59080d8ffdb9efe19c1590c9",
    "ios/FleetNotifier/Assets.xcassets/PalominoPreview.imageset/PalominoPreview-1024.png": "7e04da3407296c11f94f929124d4b6c82d83bce70e3a2700c6af906774d863ec",
    "ios/FleetNotifier/Assets.xcassets/BlackPreview.imageset/BlackPreview-1024.png": "3ff9ccc5d592a3f43c59f8fbdf09d95a75d8200666e6fc04a836babdee77e32c",
    "ios/FleetNotifier/Assets.xcassets/GreyPreview.imageset/GreyPreview-1024.png": "2d43e4c3a360ddc8d8fade528f227b8808b2bb2baaef0bb816a0373cb738402e",
    "ios/tools/herd-art/appicon-masters/treatment-a/bay-1024.png": "e9e8e7ebb922660dd76d511dd039562dbb333edc59080d8ffdb9efe19c1590c9",
    "ios/tools/herd-art/appicon-masters/treatment-a/palomino-1024.png": "7e04da3407296c11f94f929124d4b6c82d83bce70e3a2700c6af906774d863ec",
    "ios/tools/herd-art/appicon-masters/treatment-a/black-1024.png": "3ff9ccc5d592a3f43c59f8fbdf09d95a75d8200666e6fc04a836babdee77e32c",
    "ios/tools/herd-art/appicon-masters/treatment-a/grey-1024.png": "2d43e4c3a360ddc8d8fade528f227b8808b2bb2baaef0bb816a0373cb738402e",
}

INTEGRATION_SHA256 = {
    "tools/icon/from-user-png.py": "ded8b6c398aaaf17adc8af148af2bf8e001ffcbc090638b283e433a95171cab4",
    "ios/tools/herd-art/app-icons.py": "d726a9745b4ec3b48cbb93c2f6a5d441fe86b1de04aafcbd9b24e701f02390d5",
    "ios/tools/herd-art/appicon-approval.json": "3a3d907345fe1acad316c0f618a66b3b56aa7f165976cca82830627602305afb",
    "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/Contents.json": "9cf2928dad89427abd581b6852db081352417f400afb65e60c479228cdc4f4da",
    "ios/FleetNotifier/Assets.xcassets/Palomino.appiconset/Contents.json": "fa8428298e77c3d34b2f4bb634d1d52d488e89893dd11298cc6dd25b87338611",
    "ios/FleetNotifier/Assets.xcassets/Black.appiconset/Contents.json": "7cdf8e4c5e754aacdfa8bb41c878d8526f3b11c8b07012911bc8e58c8f4bade5",
    "ios/FleetNotifier/Assets.xcassets/Grey.appiconset/Contents.json": "593a117639762bf7854b3f04e52df6abce9abc50c0577d441de1f3631c685443",
    # #464: the generated loadable preview imagesets (contract-pinned).
    "ios/FleetNotifier/Assets.xcassets/BayPreview.imageset/Contents.json": "15f64a5a507fc9130e05db19dfefb65f462d3d797c3a6fe2bd11ea4baa262bf6",
    "ios/FleetNotifier/Assets.xcassets/PalominoPreview.imageset/Contents.json": "7cd7735b7052b3061ec6cb8d9099c7e79cde2414848cfe7526b88fccfae24fb5",
    "ios/FleetNotifier/Assets.xcassets/BlackPreview.imageset/Contents.json": "9b9fc329a207b2bbe4bdb7229213a30f193924d36e77797fea837c6e20e75ca5",
    "ios/FleetNotifier/Assets.xcassets/GreyPreview.imageset/Contents.json": "0462eb2d9fa5a449a3f54431cc83f2d3fbd1f2992f7345d3604eeb0704a62548",
    # Pin refreshed by the #464 lane: xcodegen regenerate now also makes the
    # app icon change visible to the test bundle through base64 fixtures
    # (encoded, so the app-product art scan never sees extra loose artwork);
    # the app product and its catalog wiring are unchanged.
    # Pin refreshed by the #456 refresh2 lane: the project is regenerated from
    # the merged ios/project.yml, so the test bundle's preBuild fixture copy
    # step now carries BOTH the #456 RanchEnvironment source fixture and the
    # #464 picker/master fixtures — the union, nothing else (diff vs the #464
    # pin = the two RanchEnvironment inputFiles/outputFiles lines + the cp
    # line). Re-pinned over the committed merged project.
    "ios/FleetNotifier.xcodeproj/project.pbxproj": "848cffee8040392299160836929bec73ffb4aebd09e90c8cbf73c75284541a92",
}

PNG_SPECS = {
    "assets/icon/corral-master.png": ((1001, 1001), "RGB"),
    "assets/icon/corral-icon-1024.png": ((1024, 1024), "RGB"),
    "assets/icon/corral-icon-256.png": ((256, 256), "RGB"),
    "assets/icon/social-preview.png": ((1280, 640), "RGB"),
    "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png": ((1024, 1024), "RGB"),
    "ios/FleetNotifier/Assets.xcassets/Palomino.appiconset/Palomino-1024.png": ((1024, 1024), "RGB"),
    "ios/FleetNotifier/Assets.xcassets/Black.appiconset/Black-1024.png": ((1024, 1024), "RGB"),
    "ios/FleetNotifier/Assets.xcassets/Grey.appiconset/Grey-1024.png": ((1024, 1024), "RGB"),
    "ios/FleetNotifier/Assets.xcassets/BayPreview.imageset/BayPreview-1024.png": ((1024, 1024), "RGB"),
    "ios/FleetNotifier/Assets.xcassets/PalominoPreview.imageset/PalominoPreview-1024.png": ((1024, 1024), "RGB"),
    "ios/FleetNotifier/Assets.xcassets/BlackPreview.imageset/BlackPreview-1024.png": ((1024, 1024), "RGB"),
    "ios/FleetNotifier/Assets.xcassets/GreyPreview.imageset/GreyPreview-1024.png": ((1024, 1024), "RGB"),
    "ios/tools/herd-art/appicon-masters/treatment-a/bay-1024.png": ((1024, 1024), "RGB"),
    "ios/tools/herd-art/appicon-masters/treatment-a/palomino-1024.png": ((1024, 1024), "RGB"),
    "ios/tools/herd-art/appicon-masters/treatment-a/black-1024.png": ((1024, 1024), "RGB"),
    "ios/tools/herd-art/appicon-masters/treatment-a/grey-1024.png": ((1024, 1024), "RGB"),
    "assets/icon/corral-icon-macos.png": ((1024, 1024), "RGBA"),
}

SHIPPING_APPICONS = (
    ("AppIcon", "bay"),
    ("Palomino", "palomino"),
    ("Black", "black"),
    ("Grey", "grey"),
)

# #464: the loadable preview imagesets backing the Settings App Icon picker.
SHIPPING_PREVIEWS = (
    ("BayPreview", "bay"),
    ("PalominoPreview", "palomino"),
    ("BlackPreview", "black"),
    ("GreyPreview", "grey"),
)

MAC_SAFE_EXTENT = 824
MAC_PLATE_SIZE = 1024
MAC_CENTER_TOLERANCE = 1

SOCIAL_BACKGROUND = (1, 1, 1)
SOCIAL_WORDMARK_STATS = (2_775, (423, 284, 622, 330), ((245, 245, 245), 2_122))
SOCIAL_CAPTION_STATS = (3_526, (425, 345, 837, 382), ((170, 170, 170), 1_542))

FIXTURE_FILES = [
    *APPROVED_SHA256,
    *INTEGRATION_SHA256,
    "ios/FleetNotifier/Info.plist",
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


def load_icon_generator(root: Path) -> ModuleType:
    path = root / "tools/icon/from-user-png.py"
    require(path.is_file(), f"missing {path.relative_to(root)}")
    spec = importlib.util.spec_from_file_location("corral_icon_generator", path)
    require(spec is not None and spec.loader is not None, "cannot load icon generator module")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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
    mac = load_png(root, "assets/icon/corral-icon-macos.png")

    # #463: every shipping app icon is pixel-identical to its approved
    # Treatment-A master. The historical repository reference art above is
    # deliberately no longer the app icon.
    for name, coat in SHIPPING_APPICONS:
        shipping = load_png(
            root, f"ios/FleetNotifier/Assets.xcassets/{name}.appiconset/{name}-1024.png"
        )
        approved = load_png(
            root, f"ios/tools/herd-art/appicon-masters/treatment-a/{coat}-1024.png"
        )
        require(
            shipping.tobytes() == approved.tobytes(),
            f"{name} app icon does not match its approved Treatment-A master",
        )

    # #464: every loadable preview is byte-identical to its approved master.
    for name, coat in SHIPPING_PREVIEWS:
        preview = load_png(
            root, f"ios/FleetNotifier/Assets.xcassets/{name}.imageset/{name}-1024.png"
        )
        approved = load_png(
            root, f"ios/tools/herd-art/appicon-masters/treatment-a/{coat}-1024.png"
        )
        require(
            preview.tobytes() == approved.tobytes(),
            f"#464 {name} preview does not match its approved Treatment-A master",
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

    generator = load_icon_generator(root)
    expected_mac = generator.macos_squircle(icon_1024)
    require(
        ImageChops.difference(mac, expected_mac).getbbox() is None,
        "macOS output does not match the squircle generator derivation",
    )
    alpha_histogram = Counter(compatible_pixel_data(mac.getchannel("A")))
    require(0 in alpha_histogram and 255 in alpha_histogram, "macOS output lacks transparent corners or opaque interior")
    require(
        all(mac.getpixel(point)[3] == 0 for point in ((0, 0), (1023, 0), (0, 1023), (1023, 1023))),
        "macOS output corners are not transparent",
    )
    require(mac.getpixel((512, 512))[3] == 255, "macOS output center is not opaque")
    mac_bbox = generator.content_bbox(mac.convert("RGB"))
    require(mac_bbox is not None, "macOS output has no visible content")
    mac_left, mac_top, mac_right, mac_bottom = mac_bbox
    mac_width = mac_right - mac_left + 1
    mac_height = mac_bottom - mac_top + 1
    require(mac_width <= MAC_SAFE_EXTENT, f"macOS content width {mac_width} exceeds safe extent")
    require(mac_height <= MAC_SAFE_EXTENT, f"macOS content height {mac_height} exceeds safe extent")
    require(
        abs((mac_left + mac_right + 1) / 2 - MAC_PLATE_SIZE / 2) <= MAC_CENTER_TOLERANCE,
        "macOS content is not horizontally centered",
    )
    require(
        abs((mac_top + mac_bottom + 1) / 2 - MAC_PLATE_SIZE / 2) <= MAC_CENTER_TOLERANCE,
        "macOS content is not vertically centered",
    )

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


def run_python_syntax(root: Path, relative: str) -> None:
    path = root / relative
    require(path.is_file(), f"missing {relative}")
    try:
        compile(path.read_text(encoding="utf-8"), str(path), "exec")
    except (OSError, SyntaxError) as error:
        raise SystemExit(f"icon check failed: {relative} has invalid Python: {error}") from error


def check_references(root: Path) -> None:
    for name, _ in SHIPPING_APPICONS:
        contents_path = (
            root / f"ios/FleetNotifier/Assets.xcassets/{name}.appiconset/Contents.json"
        )
        try:
            contents = json.loads(contents_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise SystemExit(f"icon check failed: invalid iOS {name} catalog: {error}") from error
        images = contents.get("images", [])
        expected_image = {
            "filename": f"{name}-1024.png",
            "idiom": "universal",
            "platform": "ios",
            "size": "1024x1024",
        }
        require(
            expected_image in images,
            f"iOS {name} catalog does not reference the 1024 asset",
        )
        require(
            (contents_path.parent / f"{name}-1024.png").is_file(),
            f"iOS {name} catalog points at a missing PNG",
        )

    for name, _ in SHIPPING_PREVIEWS:
        contents_path = (
            root / f"ios/FleetNotifier/Assets.xcassets/{name}.imageset/Contents.json"
        )
        try:
            contents = json.loads(contents_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise SystemExit(f"icon check failed: invalid iOS {name} preview catalog: {error}") from error
        images = contents.get("images", [])
        expected_image = {
            "filename": f"{name}-1024.png",
            "idiom": "universal",
            "scale": "1x",
        }
        require(
            expected_image in images,
            f"iOS {name} preview catalog does not reference the 1024 asset at 1x",
        )
        require(
            (contents_path.parent / f"{name}-1024.png").is_file(),
            f"iOS {name} preview catalog points at a missing PNG",
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
    require(
        re.search(
            r'ASSETCATALOG_COMPILER_ALTERNATE_APPICON_NAMES\s*=\s*"Palomino Black Grey";',
            project,
        ),
        "Xcode project does not select the approved alternate app icon sets",
    )
    require(
        re.search(r'TARGETED_DEVICE_FAMILY\s*=\s*"1,2";', project),
        "Xcode project does not declare the iPhone+iPad device family",
    )

    run_python_syntax(root, "tools/icon/from-user-png.py")


def check_all(root: Path) -> None:
    check_hashes(root)
    check_integration_hashes(root)
    check_pillow_compatibility()
    check_pixels(root)
    check_references(root)


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


def expect_image_rejection(
    label: str,
    relative: str,
    mutate: Callable[[Path], None],
) -> None:
    original_sha = APPROVED_SHA256[relative]
    try:
        with tempfile.TemporaryDirectory(prefix="corral-icon-check-") as temporary:
            fixture = make_fixture(Path(temporary))
            check_all(fixture)
            mutate(fixture)
            path = fixture / relative
            APPROVED_SHA256[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
            try:
                check_all(fixture)
            except SystemExit:
                return
            raise AssertionError(f"self-test mutation was accepted: {label}")
    finally:
        APPROVED_SHA256[relative] = original_sha


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


def mutate_mac_corner_alpha(root: Path) -> None:
    path = root / "assets/icon/corral-icon-macos.png"
    with Image.open(path) as source:
        image = source.copy()
    red, green, blue, _ = image.getpixel((0, 0))
    image.putpixel((0, 0), (red, green, blue, 255))
    image.save(path)


def mutate_mac_unpadded(root: Path) -> None:
    source = root / "assets/icon/corral-icon-1024.png"
    target = root / "assets/icon/corral-icon-macos.png"
    with Image.open(source) as image:
        image.convert("RGBA").save(target)


def mutate_mac_off_center(root: Path) -> None:
    path = root / "assets/icon/corral-icon-macos.png"
    with Image.open(path) as source:
        image = source.copy()
    shifted = Image.new("RGBA", image.size, (1, 1, 1, 255))
    shifted.paste(image.convert("RGB"), (16, 0))
    shifted.save(path)


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


def mutate_alternate_catalog(root: Path) -> None:
    path = root / "ios/FleetNotifier/Assets.xcassets/Palomino.appiconset/Contents.json"
    contents = json.loads(path.read_text(encoding="utf-8"))
    contents["images"][0]["filename"] = "Palomino-512.png"
    path.write_text(json.dumps(contents), encoding="utf-8")


def mutate_missing_alternate(root: Path) -> None:
    shutil.rmtree(root / "ios/FleetNotifier/Assets.xcassets/Grey.appiconset")


def mutate_preview_catalog(root: Path) -> None:
    path = root / "ios/FleetNotifier/Assets.xcassets/BayPreview.imageset/Contents.json"
    contents = json.loads(path.read_text(encoding="utf-8"))
    contents["images"][0]["scale"] = "2x"
    path.write_text(json.dumps(contents), encoding="utf-8")


def mutate_missing_preview(root: Path) -> None:
    shutil.rmtree(root / "ios/FleetNotifier/Assets.xcassets/GreyPreview.imageset")


def mutate_project_alternates(root: Path) -> None:
    path = root / "ios/FleetNotifier.xcodeproj/project.pbxproj"
    source = path.read_text(encoding="utf-8")
    line = '\t\t\t\tASSETCATALOG_COMPILER_ALTERNATE_APPICON_NAMES = "Palomino Black Grey";\n'
    require(line in source, "self-test fixture could not find the alternate app icon setting")
    path.write_text(source.replace(line, "", 1), encoding="utf-8")


def mutate_generator_noop(root: Path) -> None:
    path = root / "tools/icon/from-user-png.py"
    source = path.read_text(encoding="utf-8")
    path.write_text(
        source.replace("def main() -> None:\n", "def main() -> None:\n    return\n", 1),
        encoding="utf-8",
    )


def mutate_generator_hash(root: Path) -> None:
    path = root / "tools/icon/from-user-png.py"
    data = bytearray(path.read_bytes())
    data[len(data) // 2] ^= 1
    path.write_bytes(data)


def mutate_appicon_resources_phase(root: Path) -> None:
    path = root / "ios/FleetNotifier.xcodeproj/project.pbxproj"
    source = path.read_text(encoding="utf-8")
    resource_line = "\t\t\t\t21CE07E2350DAFAF99DDC395 /* Assets.xcassets in Resources */,\n"
    require(resource_line in source, "self-test fixture could not find the AppIcon resource phase entry")
    path.write_text(source.replace(resource_line, "", 1), encoding="utf-8")


def mutate_generator_padding(root: Path) -> None:
    path = root / "tools/icon/from-user-png.py"
    source = path.read_text(encoding="utf-8")
    path.write_text(source.replace("MAC_SAFE_EXTENT = 824", "MAC_SAFE_EXTENT = 1024", 1), encoding="utf-8")


def mutate_generator_fallback(root: Path) -> None:
    path = root / "tools/icon/from-user-png.py"
    source = path.read_text(encoding="utf-8")
    path.write_text(
        source.replace("ImageFont.truetype", "ImageFont.load_default"),
        encoding="utf-8",
    )


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
            ("social wordmark corruption", mutate_social_wordmark),
            ("social caption corruption", mutate_social_caption),
            ("iOS AppIcon catalog filename", mutate_appicon_catalog),
            ("iOS alternate catalog filename", mutate_alternate_catalog),
            ("iOS alternate appiconset removed", mutate_missing_alternate),
            ("iOS preview catalog scale", mutate_preview_catalog),
            ("iOS preview imageset removed", mutate_missing_preview),
            ("alternate app icon sets removed from project", mutate_project_alternates),
            ("approved master bytes", mutate_bytes(
                "ios/tools/herd-art/appicon-masters/treatment-a/bay-1024.png")),
            ("immediate-return generator", mutate_generator_noop),
            ("generator source hash", mutate_generator_hash),
            ("AppIcon removed from Resources phase", mutate_appicon_resources_phase),
            ("generator padding extent", mutate_generator_padding),
            ("generator font fallback", mutate_generator_fallback),
        ]
    )
    for label, mutation in mutations:
        expect_rejection(label, mutation)

    for label, mutation in [
        ("macOS alpha hole", mutate_mac_alpha),
        ("macOS transparent corner", mutate_mac_corner_alpha),
        ("macOS missing padding", mutate_mac_unpadded),
        ("macOS off-center glyph", mutate_mac_off_center),
    ]:
        expect_image_rejection(
            label,
            "assets/icon/corral-icon-macos.png",
            mutation,
        )

    print(f"icon checker self-tests: ok ({len(mutations) + 1} negative cases)")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run negative fixture tests in temporary directories",
    )
    args = parser.parse_args()

    if args.self_test:
        self_test()
    else:
        check_all(ROOT)
        print("icon assets and integration references: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
