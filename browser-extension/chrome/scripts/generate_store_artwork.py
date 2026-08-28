"""Regenerate checked-in store artwork; requires Pillow 11.3.0."""

from pathlib import Path

from PIL import Image, ImageOps


ROOT = Path(__file__).resolve().parents[1]
SIZES = (16, 32, 48, 128)


def main() -> None:
    output = ROOT / "public" / "icons"
    output.mkdir(parents=True, exist_ok=True)
    store = ROOT / "store" / "assets"
    store.mkdir(parents=True, exist_ok=True)
    with Image.open(ROOT / "artwork" / "browser-icon-source.png") as source:
        image = source.convert("RGBA")
        for size in SIZES:
            # Store artwork has a 96px image area and 16px transparent padding.
            inset = 16 if size == 128 else 0
            icon = Image.new("RGBA", (size, size), (0, 0, 0, 0))
            scaled = image.resize((size - 2 * inset, size - 2 * inset), Image.Resampling.LANCZOS)
            icon.alpha_composite(scaled, (inset, inset))
            icon.save(output / f"icon-{size}.png", optimize=False, compress_level=9)
        image.resize((300, 300), Image.Resampling.LANCZOS).save(
            store / "logo-300.png", optimize=False, compress_level=9
        )
    with Image.open(ROOT / "artwork" / "promo-source.png") as source:
        tile = ImageOps.fit(source.convert("RGB"), (440, 280), Image.Resampling.LANCZOS)
        tile.save(store / "promo-440x280.png", optimize=False, compress_level=9)


if __name__ == "__main__":
    main()
