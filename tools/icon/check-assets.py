#!/usr/bin/env python3
"""Validate the approved Corral icon outputs and their integrations.

The default check is read-only and does not build or install anything. It
pins the approved bytes as well as checking image structure, generator
relationships, parsed project metadata, shell syntax, and the egui source
tokens that make the icon application real code rather than a comment.

Use ``--require-build`` after a release build to prove that the 256px PNG is
present in the produced ``corrald-ui`` executable. ``--self-test`` runs the
same checks against temporary fixtures with deliberate corruptions.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
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
    "tools/icon/from-user-png.py",
]


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"icon check failed: {message}")


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


def check_hashes(root: Path) -> None:
    for relative, expected in APPROVED_SHA256.items():
        path = root / relative
        require(path.is_file(), f"missing {relative}")
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        require(actual == expected, f"{relative} does not match the approved SHA-256")


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
    alpha_histogram = Counter(mac.getchannel("A").get_flattened_data())
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


def rust_tokens(source: str) -> list[tuple[str, str]]:
    """Tokenize enough Rust to distinguish code from comments and strings."""

    tokens: list[tuple[str, str]] = []
    index = 0
    length = len(source)
    while index < length:
        if source.startswith("//", index):
            newline = source.find("\n", index + 2)
            index = length if newline == -1 else newline + 1
            continue
        if source.startswith("/*", index):
            depth = 1
            index += 2
            while index < length and depth:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            continue
        if source[index] == '"':
            start = index + 1
            index = start
            escaped = False
            while index < length:
                character = source[index]
                if character == '"' and not escaped:
                    break
                if character == "\\" and not escaped:
                    escaped = True
                else:
                    escaped = False
                index += 1
            tokens.append(("string", source[start:index]))
            index = min(index + 1, length)
            continue
        raw_match = re.match(r'r(#+)"', source[index:])
        if raw_match:
            hashes = raw_match.group(1)
            start = index + len(raw_match.group(0))
            terminator = '"' + hashes
            end = source.find(terminator, start)
            if end == -1:
                end = length
            tokens.append(("string", source[start:end]))
            index = min(end + len(terminator), length)
            continue
        identifier = re.match(r"[A-Za-z_][A-Za-z0-9_]*", source[index:])
        if identifier:
            value = identifier.group(0)
            tokens.append(("ident", value))
            index += len(value)
            continue
        if source[index].isspace():
            index += 1
            continue
        tokens.append(("punct", source[index]))
        index += 1
    return tokens


def has_tokens(tokens: list[tuple[str, str]], expected: list[tuple[str, str]]) -> bool:
    width = len(expected)
    return any(tokens[index : index + width] == expected for index in range(len(tokens)))


def check_egui_source(root: Path) -> None:
    source = read_text(root, "clients/egui/src/main.rs")
    tokens = rust_tokens(source)
    include = [
        ("ident", "include_bytes"),
        ("punct", "!"),
        ("punct", "("),
        ("string", "../../../assets/icon/corral-icon-256.png"),
        ("punct", ")"),
    ]
    require(has_tokens(tokens, include), "egui does not embed the 256 icon in code")
    require(
        has_tokens(
            tokens,
            [
                ("punct", "."),
                ("ident", "with_icon"),
                ("punct", "("),
                ("ident", "app_icon"),
                ("punct", "("),
                ("punct", ")"),
                ("punct", ")"),
            ],
        ),
        "egui viewport does not apply the embedded icon in code",
    )


def check_generator(root: Path) -> None:
    relative = "tools/icon/from-user-png.py"
    source = read_text(root, relative)
    try:
        tree = ast.parse(source, filename=relative)
    except SyntaxError as error:
        raise SystemExit(f"icon check failed: generator syntax error: {error}") from error

    strings = {
        node.value
        for node in ast.walk(tree)
        if isinstance(node, ast.Constant) and isinstance(node.value, str)
    }
    attributes = {
        node.attr for node in ast.walk(tree) if isinstance(node, ast.Attribute)
    }
    calls = {
        node.func.id
        for node in ast.walk(tree)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
    }
    called_attributes = {
        node.func.attr
        for node in ast.walk(tree)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
    }
    names = {node.id for node in ast.walk(tree) if isinstance(node, ast.Name)}

    require("assets/icon" in strings, "generator does not use the approved icon output directory")
    for output in (
        "corral-icon-macos.png",
        "corral-icon-1024.png",
        "corral-icon-256.png",
        "social-preview.png",
    ):
        require(output in strings, f"generator does not write {output}")
    for required_attribute in ("BOX", "rounded_rectangle", "putalpha", "truetype"):
        require(
            required_attribute in attributes or required_attribute in called_attributes,
            f"generator is missing deterministic {required_attribute} processing",
        )
    require(
        "sha256" in calls or "sha256" in called_attributes,
        "generator does not fingerprint its required font",
    )
    require("load_default" not in calls, "generator silently falls back to a different font")
    require(
        "APPROVED_WORDMARK_FONT_SHA256" in names,
        "generator does not pin the approved wordmark font fingerprint",
    )
    require(
        "2bfd40dc72e6759e248f82a52a40d551338979fffc9b5c070e685b4b7ad19e66" in strings,
        "generator is missing the approved wordmark font SHA-256",
    )


def run_bash_syntax(root: Path, relative: str) -> None:
    path = root / relative
    require(path.is_file(), f"missing {relative}")
    result = subprocess.run(
        ["bash", "-n", str(path)], capture_output=True, text=True, check=False
    )
    require(result.returncode == 0, f"{relative} fails bash -n: {result.stderr.strip()}")


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
    require(
        re.search(r"PBXResourcesBuildPhase[^;]*Assets\.xcassets", project, re.DOTALL)
        or "Assets.xcassets in Resources" in project,
        "Xcode project omits Assets.xcassets from resources",
    )
    require(
        re.search(r"ASSETCATALOG_COMPILER_APPICON_NAME\s*=\s*AppIcon;", project),
        "Xcode project does not select AppIcon",
    )

    check_egui_source(root)

    setup = read_text(root, "scripts/setup-corrald.sh")
    require(
        re.search(
            r"bash\s+\"\$REPO_DIR/scripts/install-corral-ui\.sh\"\s+--binary\s+\"\$UI_BIN\"",
            setup,
        ),
        "setup script does not invoke the transactional desktop installer",
    )
    helper = read_text(root, "scripts/install-corral-ui.sh")
    for pattern, message in (
        (r"install_macos\s*\(\)", "macOS installer function is missing"),
        (r"iconutil\s+-c\s+icns", "macOS installer does not create an icns"),
        (r"CFBundleIconFile.*Corral", "macOS bundle does not declare CFBundleIconFile"),
        (
            r"commit_directory\s+\"\$stage\"\s+\"\$MACOS_APP_DEST\"",
            "macOS installer is not transactional",
        ),
        (r"install_linux\s*\(\)", "Linux installer function is missing"),
        (r"corral-icon-256\.png", "Linux installer does not use the approved icon"),
        (r"Icon=corral", "Linux desktop entry does not reference corral"),
        (r"commit_files", "Linux installer has no transactional payload commit"),
    ):
        require(re.search(pattern, helper, re.DOTALL) is not None, message)
    run_bash_syntax(root, "scripts/setup-corrald.sh")
    run_bash_syntax(root, "scripts/install-corral-ui.sh")
    run_bash_syntax(root, "scripts/test-icon-packaging.sh")


def check_build_embedding(root: Path) -> None:
    binary = root / "target/release/corrald-ui"
    require(binary.is_file(), "release corrald-ui binary is missing; run cargo build --release")
    icon = (root / "assets/icon/corral-icon-256.png").read_bytes()
    executable = binary.read_bytes()
    require(icon in executable, "release corrald-ui does not contain the approved 256 icon bytes")


def check_all(root: Path, require_build: bool = False) -> None:
    check_hashes(root)
    check_pixels(root)
    check_references(root)
    check_generator(root)
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
