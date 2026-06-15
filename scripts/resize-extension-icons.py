#!/usr/bin/env python3
"""Regenerate flint-extension toolbar icons from the master CV mark."""

from __future__ import annotations

from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
MASTER = ROOT / "src/assets/flint-logo-extension.png"
OUT_DIR = ROOT.parent / "flint-extension/icons"


def square_crop(img: Image.Image) -> Image.Image:
    w, h = img.size
    side = min(w, h)
    left = (w - side) // 2
    top = (h - side) // 2
    return img.crop((left, top, left + side, top + side))


def main() -> None:
    if not MASTER.is_file():
        raise SystemExit(f"Master icon missing: {MASTER}")
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    master = square_crop(Image.open(MASTER).convert("RGBA"))
    for size in (16, 32, 48, 128):
        out = OUT_DIR / f"icon{size}.png"
        master.resize((size, size), Image.Resampling.LANCZOS).save(out, "PNG", optimize=True)
        print(f"wrote {out}")


if __name__ == "__main__":
    main()
