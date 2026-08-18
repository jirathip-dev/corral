#!/usr/bin/env python3
"""Transform the user's approved icon PNG into every required size.

Source: the user's image (horse head + isometric pen, silver-on-black).
Process:
  1. Load the source (1254x1254 RGB, near-black bg).
  2. Auto-crop to content (threshold on near-black) + keep a ~9% safe
     margin so the subject fills the icon without touching the squircle.
  3. Render to: iOS 1024 opaque, egui 256, repo 1024, social preview.

Usage: python3 tools/icon/from-user-png.py <input.png>
"""
import os, subprocess, sys
from PIL import Image, ImageDraw

def content_bbox(img, bg_thresh=18):
    """Bounding box of non-background pixels (near-black = bg)."""
    w, h = img.size
    gray = img.convert("L")
    px = gray.load()
    minx, miny, maxx, maxy = w, h, -1, -1
    for y in range(0, h, 2):
        for x in range(0, w, 2):
            if px[x, y] > bg_thresh:
                if x < minx: minx = x
                if x > maxx: maxx = x
                if y < miny: miny = y
                if y > maxy: maxy = y
    # refine with full-res scan on the found region edges
    for y in range(miny, maxy + 1):
        for x in range(0, w, 1):
            if px[x, y] > bg_thresh:
                if x < minx: minx = x
                break
        for x in range(w - 1, -1, -1):
            if px[x, y] > bg_thresh:
                if x > maxx: maxx = x
                break
    for x in range(minx, maxx + 1):
        for y in range(0, h, 1):
            if px[x, y] > bg_thresh:
                if y < miny: miny = y
                break
        for y in range(h - 1, -1, -1):
            if px[x, y] > bg_thresh:
                if y > maxy: maxy = y
                break
    return minx, miny, maxx, maxy

def main():
    if len(sys.argv) < 2:
        print("usage: python3 tools/icon/from-user-png.py <input.png>")
        sys.exit(1)
    src = sys.argv[1]
    img = Image.open(src).convert("RGB")
    print(f"source: {img.size}")

    # 1. crop to content + ~9% safe margin
    minx, miny, maxx, maxy = content_bbox(img)
    cw, ch = maxx - minx + 1, maxy - miny + 1
    margin = int(max(cw, ch) * 0.09)
    L = max(minx - margin, 0)
    T = max(miny - margin, 0)
    R = min(maxx + margin, img.width - 1)
    B = min(maxy + margin, img.height - 1)
    cropped = img.crop((L, T, R, B))
    # pad to square
    side = max(cropped.size)
    sq = Image.new("RGB", (side, side), (1, 1, 1))
    sq.paste(cropped, ((side - cropped.width) // 2, (side - cropped.height) // 2))
    print(f"content bbox: {minx,miny,maxx,maxy} -> cropped {cropped.size} -> square {sq.size}")

    os.makedirs("assets/icon", exist_ok=True)

    # 2. iOS AppIcon 1024 opaque
    ios = sq.resize((1024, 1024), Image.LANCZOS).convert("RGB")
    ap = "ios/FleetNotifier/Assets.xcassets/AppIcon.appiconset"
    os.makedirs(ap, exist_ok=True)
    ios.save(os.path.join(ap, "AppIcon-512@2x.png"))
    print("  ✓ iOS AppIcon (1024 opaque)")

    # 3. egui 256
    eg = sq.resize((256, 256), Image.LANCZOS).convert("RGB")
    eg.save("assets/icon/corral-icon-256.png")
    print("  ✓ egui icon (256)")

    # 4. repo reference 1024
    ios.save("assets/icon/corral-icon-1024.png")
    print("  ✓ repo reference (1024)")

    # 5. social preview 1280x640: icon + wordmark
    W, H = 1280, 640
    pv = Image.new("RGB", (W, H), (1, 1, 1))
    d = ImageDraw.Draw(pv)
    icon = sq.resize((300, 300), Image.LANCZOS)
    pv.paste(icon, (70, (H - 300) // 2))
    from PIL import ImageFont
    try:
        f1 = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 88)
        f2 = ImageFont.truetype("/System/Library/Fonts/SFNS.ttf", 30)
    except Exception:
        f1 = ImageFont.load_default(); f2 = ImageFont.load_default()
    d.text((420, H // 2 - 60), "corral", font=f1, fill=(245, 245, 245))
    d.text((424, H // 2 + 28), "control plane for your agent fleet", font=f2, fill=(170, 170, 170))
    pv.save("assets/icon/social-preview.png")
    print("  ✓ social preview (1280x640)")

    # 6. keep the source as the master reference
    sq.save("assets/icon/corral-master.png")
    print("  ✓ master (square, cropped)")

    print("done.")

if __name__ == "__main__":
    main()
