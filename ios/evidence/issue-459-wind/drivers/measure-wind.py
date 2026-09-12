#!/usr/bin/env python3
"""#459 wind evidence measurement — numeric motion analysis of recorded clips.

Pure stdlib + ffmpeg (no third-party modules). Two modes:

  clip  — extract frames from a recorded video (downscaled, stated) and report
          changed-pixel counts for consecutive and half-wind-cycle pairs plus
          a per-row motion profile and a diff-map PNG.
  pair  — compare two full-resolution screenshots (same analysis, full res).

The numbers are honest measurements of real captured pixels from the app
running on the simulator, not of an IDEALIZED renderer.
"""
import argparse
import json
import subprocess
import sys
import zlib
from pathlib import Path


def raw_frames(source: Path, width: int, height: int, *, fps: float | None = None,
               start: float = 0, duration: float = 0, video: bool = False) -> list[bytes]:
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error"]
    if video:
        cmd += ["-ss", str(start), "-t", str(duration)]
    cmd += ["-i", str(source)]
    filters = []
    if video and fps:
        filters.append(f"fps={fps}")
    if width and height:
        filters.append(f"scale={width}:{height}")
    if filters:
        cmd += ["-vf", ",".join(filters)]
    cmd += ["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    raw = subprocess.run(cmd, check=True, capture_output=True).stdout
    if not width or not height:
        return [raw] if raw else []
    frame_size = width * height * 3
    if len(raw) % frame_size:
        raise SystemExit(f"raw stream is not frame aligned ({len(raw)} bytes)")
    return [raw[i:i + frame_size] for i in range(0, len(raw), frame_size)]


def changed_pixels(one: bytes, two: bytes, threshold: int = 8) -> int:
    count = 0
    for index in range(0, len(one), 3):
        if (abs(one[index] - two[index]) > threshold
                or abs(one[index + 1] - two[index + 1]) > threshold
                or abs(one[index + 2] - two[index + 2]) > threshold):
            count += 1
    return count


def diff_bytes(one: bytes, two: bytes, gain: int = 3) -> bytes:
    out = bytearray(len(one))
    for index in range(0, len(one), 3):
        peak = max(abs(one[index] - two[index]),
                   abs(one[index + 1] - two[index + 1]),
                   abs(one[index + 2] - two[index + 2]))
        value = min(255, peak * gain)
        out[index] = out[index + 1] = out[index + 2] = value
    return bytes(out)


def write_png(path: Path, width: int, height: int, rgb: bytes) -> None:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (len(payload).to_bytes(4, "big") + kind + payload
                + zlib.crc32(kind + payload).to_bytes(4, "big"))
    scanlines = b"".join(b"\x00" + rgb[row * width * 3:(row + 1) * width * 3]
                         for row in range(height))
    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", width.to_bytes(4, "big") + height.to_bytes(4, "big")
                   + bytes([8, 2, 0, 0, 0]))
           + chunk(b"IDAT", zlib.compress(scanlines, 9))
           + chunk(b"IEND", b""))
    path.write_bytes(png)


def row_profile(one: bytes, two: bytes, width: int, height: int, band: int) -> list[dict]:
    mask = bytearray(width * height)
    for index in range(0, len(one), 3):
        if (abs(one[index] - two[index]) > 8
                or abs(one[index + 1] - two[index + 1]) > 8
                or abs(one[index + 2] - two[index + 2]) > 8):
            mask[index // 3] = 1
    profile = []
    for top in range(0, height, band):
        bottom = min(height, top + band)
        total = mask[top * width:bottom * width]
        profile.append({"rows": [top, bottom],
                        "changed_fraction": round(sum(total) / max(1, len(total)), 5)})
    return profile


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="mode", required=True)
    clip = sub.add_parser("clip")
    clip.add_argument("video", type=Path)
    clip.add_argument("--start", type=float, default=0.0)
    clip.add_argument("--duration", type=float, default=10.0)
    clip.add_argument("--fps", type=float, default=2.5)
    clip.add_argument("--width", type=int, default=294)
    clip.add_argument("--height", type=int, default=639)
    clip.add_argument("--cycle-step", type=float, default=2.6,
                      help="seconds between the pair used as one wind half-cycle")
    clip.add_argument("--label", default="clip")
    clip.add_argument("--outdir", type=Path, required=True)
    pair = sub.add_parser("pair")
    pair.add_argument("one", type=Path)
    pair.add_argument("two", type=Path)
    pair.add_argument("--band", type=int, default=60)
    pair.add_argument("--label", default="pair")
    pair.add_argument("--outdir", type=Path, required=True)
    args = parser.parse_args()
    args.outdir.mkdir(parents=True, exist_ok=True)

    if args.mode == "clip":
        frames = raw_frames(args.video, args.width, args.height, fps=args.fps,
                            start=args.start, duration=args.duration, video=True)
        if len(frames) < 2:
            raise SystemExit("not enough frames")
        width, height = args.width, args.height
        result = {"label": args.label, "mode": "clip", "video": str(args.video),
                  "window": {"start": args.start, "duration": args.duration, "fps": args.fps},
                  "resolution": {"width": width, "height": height}, "frames": len(frames)}
        consecutive = [changed_pixels(frames[i], frames[i + 1]) for i in range(len(frames) - 1)]
        result["consecutive_changed_pixels"] = {
            "mean": round(sum(consecutive) / len(consecutive), 1),
            "max": max(consecutive), "min": min(consecutive),
            "of_pixels": width * height}
        step = max(1, int(round(args.cycle_step * args.fps)))
        step = min(step, len(frames) - 1)
        a, b = frames[0], frames[step]
        changed = changed_pixels(a, b)
        result["half_cycle_pair"] = {"frame_gap": step, "changed_pixels": changed,
                                     "of_pixels": width * height,
                                     "changed_fraction": round(changed / (width * height), 5)}
        result["row_profile_half_cycle"] = row_profile(a, b, width, height, band=45)
    else:
        one = raw_frames(args.one, 0, 0, video=False)[0]
        two = raw_frames(args.two, 0, 0, video=False)[0]
        if len(one) != len(two):
            raise SystemExit("screenshots differ in size")
        pixels = len(one) // 3
        header = args.one.read_bytes()[:24]
        if header[:8] != b"\x89PNG\r\n\x1a\n":
            raise SystemExit("expected PNG screenshots")
        width = int.from_bytes(header[16:20], "big")
        height = int.from_bytes(header[20:24], "big")
        if width * height != pixels:
            raise SystemExit("PNG header does not match decoded pixels")
        changed = changed_pixels(one, two)
        result = {"label": args.label, "mode": "pair", "one": str(args.one), "two": str(args.two),
                  "resolution": {"width": width, "height": height},
                  "changed_pixels": changed, "of_pixels": pixels,
                  "changed_fraction": round(changed / pixels, 5)}
        result["row_profile"] = row_profile(one, two, width, height, band=args.band)
        write_png(args.outdir / f"{args.label}-diff.png", width, height,
                  diff_bytes(one, two))

    out_json = args.outdir / f"{args.label}-measurement.json"
    out_json.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
