#!/usr/bin/env python3
"""Transform the user's approved icon PNG into every required size.

Source: the user's image (horse head + isometric pen, silver-on-black).
Process:
  1. Load the source (1254x1254 RGB, near-black bg).
  2. Auto-crop to content (threshold on near-black) + keep a ~9% safe
     margin so the subject fills the icon without touching the squircle.
  3. Render to: iOS 1024 opaque, egui 256, macOS 1024 transparent squircle,
     repo 1024, social preview.

Usage: python3 tools/icon/from-user-png.py <input.png>

The approved social preview uses Apple's SFNS.ttf. It is not bundled because
the system font is not a repository asset; regeneration therefore requires
that exact font (or an explicitly supplied file with the same fingerprint).
There is deliberately no typography fallback.
"""

from __future__ import annotations

import hashlib
import math
import os
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


APPROVED_WORDMARK_FONT = "/System/Library/Fonts/SFNS.ttf"
APPROVED_WORDMARK_FONT_SHA256 = (
    "2bfd40dc72e6759e248f82a52a40d551338979fffc9b5c070e685b4b7ad19e66"
)
WORDMARK_FONT_ENV = "CORRAL_ICON_FONT"
MAC_SAFE_EXTENT = 824
MAC_PLATE_SIZE = 1024
MAC_SQUIRCLE_EXTENT = 960
MAC_SQUIRCLE_EXPONENT = 5
MAC_PLATE_BACKGROUND = (1, 1, 1)


def content_bbox(img: Image.Image, bg_thresh: int = 18) -> tuple[int, int, int, int]:
    """Return the bounding box of non-background pixels."""

    width, height = img.size
    gray = img.convert("L")
    pixels = gray.load()
    min_x, min_y, max_x, max_y = width, height, -1, -1
    for y in range(0, height, 2):
        for x in range(0, width, 2):
            if pixels[x, y] > bg_thresh:
                min_x = min(min_x, x)
                max_x = max(max_x, x)
                min_y = min(min_y, y)
                max_y = max(max_y, y)

    # Refine with full-resolution scans on the found region's edges.
    for y in range(min_y, max_y + 1):
        for x in range(width):
            if pixels[x, y] > bg_thresh:
                min_x = min(min_x, x)
                break
        for x in range(width - 1, -1, -1):
            if pixels[x, y] > bg_thresh:
                max_x = max(max_x, x)
                break
    for x in range(min_x, max_x + 1):
        for y in range(height):
            if pixels[x, y] > bg_thresh:
                min_y = min(min_y, y)
                break
        for y in range(height - 1, -1, -1):
            if pixels[x, y] > bg_thresh:
                max_y = max(max_y, y)
                break
    return min_x, min_y, max_x, max_y


def macos_squircle(icon_1024: Image.Image) -> Image.Image:
    """Return the artwork with a deterministic transparent macOS silhouette.

    The artwork is uniformly fitted to the existing 824px safe region. The
    outer plate is a centered fifth-power superellipse, matching the rounded
    squircle used by macOS while keeping its corners genuinely transparent.
    """

    if icon_1024.size != (MAC_PLATE_SIZE, MAC_PLATE_SIZE):
        raise ValueError(f"expected a {MAC_PLATE_SIZE}x{MAC_PLATE_SIZE} plate, got {icon_1024.size}")

    source = icon_1024.convert("RGB")
    min_x, min_y, max_x, max_y = content_bbox(source)
    max_extent = max(max_x - min_x + 1, max_y - min_y + 1)
    side = min(MAC_PLATE_SIZE, round(MAC_PLATE_SIZE * MAC_SAFE_EXTENT / max_extent))
    subject = source.resize((side, side), Image.Resampling.LANCZOS) if side != MAC_PLATE_SIZE else source
    plate = Image.new("RGBA", (MAC_PLATE_SIZE, MAC_PLATE_SIZE), (*MAC_PLATE_BACKGROUND, 0))
    offset = ((MAC_PLATE_SIZE - side) // 2, (MAC_PLATE_SIZE - side) // 2)
    plate.paste(subject, offset)

    # Supersampling leaves a stable antialiased edge instead of jagged corners.
    scale = 4
    mask = Image.new("L", (MAC_PLATE_SIZE * scale, MAC_PLATE_SIZE * scale), 0)
    center = MAC_PLATE_SIZE * scale / 2
    half = MAC_SQUIRCLE_EXTENT * scale / 2
    points = []
    for index in range(257):
        angle = 2 * math.pi * index / 256
        x = abs(math.cos(angle)) ** (2 / MAC_SQUIRCLE_EXPONENT)
        y = abs(math.sin(angle)) ** (2 / MAC_SQUIRCLE_EXPONENT)
        points.append((center + half * math.copysign(x, math.cos(angle)), center + half * math.copysign(y, math.sin(angle))))
    ImageDraw.Draw(mask).polygon(points, fill=255)
    mask = mask.resize((MAC_PLATE_SIZE, MAC_PLATE_SIZE), Image.Resampling.LANCZOS)
    plate.putalpha(mask)
    return plate


def approved_wordmark_fonts() -> tuple[ImageFont.FreeTypeFont, ImageFont.FreeTypeFont]:
    """Load the pinned wordmark font or fail loudly before writing outputs."""

    font_path = Path(os.environ.get(WORDMARK_FONT_ENV, APPROVED_WORDMARK_FONT))
    if not font_path.is_file():
        raise SystemExit(
            f"wordmark font missing: {font_path}; install the approved SFNS.ttf "
            f"or set {WORDMARK_FONT_ENV} to the same approved font bytes"
        )
    actual_hash = hashlib.sha256(font_path.read_bytes()).hexdigest()
    if actual_hash != APPROVED_WORDMARK_FONT_SHA256:
        raise SystemExit(
            f"wordmark font fingerprint mismatch for {font_path}: "
            f"expected {APPROVED_WORDMARK_FONT_SHA256}, got {actual_hash}"
        )
    try:
        return (
            ImageFont.truetype(str(font_path), 88),
            ImageFont.truetype(str(font_path), 30),
        )
    except OSError as error:
        raise SystemExit(f"could not load approved wordmark font {font_path}: {error}") from error


def main() -> None:
    if len(sys.argv) < 2:
        print("usage: python3 tools/icon/from-user-png.py <input.png>")
        raise SystemExit(1)

    source = Path(sys.argv[1])
    with Image.open(source) as source_image:
        image = source_image.convert("RGB")
    print(f"source: {image.size}")

    # 1. Crop to content + ~9% safe margin.
    min_x, min_y, max_x, max_y = content_bbox(image)
    content_width = max_x - min_x + 1
    content_height = max_y - min_y + 1
    margin = int(max(content_width, content_height) * 0.09)
    left = max(min_x - margin, 0)
    top = max(min_y - margin, 0)
    right = min(max_x + margin, image.width - 1)
    bottom = min(max_y + margin, image.height - 1)
    cropped = image.crop((left, top, right, bottom))
    side = max(cropped.size)
    square = Image.new("RGB", (side, side), (1, 1, 1))
    square.paste(cropped, ((side - cropped.width) // 2, (side - cropped.height) // 2))
    print(
        f"content bbox: {min_x,min_y,max_x,max_y} -> "
        f"cropped {cropped.size} -> square {square.size}"
    )

    # Validate the typography dependency before creating or changing outputs.
    wordmark_font, caption_font = approved_wordmark_fonts()

    assets_dir = Path("assets/icon")
    assets_dir.mkdir(parents=True, exist_ok=True)

    # 2. iOS AppIcon 1024 opaque.
    ios = square.resize((1024, 1024), Image.LANCZOS).convert("RGB")
    appicon_dir = Path("ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset")
    appicon_dir.mkdir(parents=True, exist_ok=True)
    ios.save(appicon_dir / "AppIcon-512@2x.png")
    print("  ✓ iOS AppIcon (1024 opaque)")

    # 3. egui/Linux icon 256.
    egui = square.resize((256, 256), Image.LANCZOS).convert("RGB")
    egui.save(assets_dir / "corral-icon-256.png")
    print("  ✓ egui icon (256)")

    # 3b. macOS app icon: bake the rounded squircle and transparency.
    mac = macos_squircle(ios)
    mac.save(assets_dir / "corral-icon-macos.png")
    print("  ✓ macOS app icon (1024 transparent squircle)")

    # 4. Repository reference 1024.
    ios.save(assets_dir / "corral-icon-1024.png")
    print("  ✓ repo reference (1024)")

    # 5. Social preview 1280x640: icon + wordmark.
    width, height = 1280, 640
    preview = Image.new("RGB", (width, height), (1, 1, 1))
    draw = ImageDraw.Draw(preview)
    social_icon = square.resize((300, 300), Image.LANCZOS)
    preview.paste(social_icon, (70, (height - 300) // 2))
    draw.text((420, height // 2 - 60), "corral", font=wordmark_font, fill=(245, 245, 245))
    draw.text(
        (424, height // 2 + 28),
        "control plane for your agent fleet",
        font=caption_font,
        fill=(170, 170, 170),
    )
    preview.save(assets_dir / "social-preview.png")
    print("  ✓ social preview (1280x640)")

    # 6. Keep the source as the master reference.
    square.save(assets_dir / "corral-master.png")
    print("  ✓ master (square, cropped)")
    print("done.")


if __name__ == "__main__":
    main()
