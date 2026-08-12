#!/usr/bin/env python3
"""Generate the application icons.

Kept as a script rather than as opaque binaries with no provenance: the two
PNGs and the ICO beside it are exactly what this produces. Standard library
only, like the rest of the project.

    python3 make-icons.py
"""
import struct
import zlib

BACKGROUND = (0x9A, 0x2C, 0x3F, 0xFF)  # the accent colour the docs site uses
CARD = (0xFF, 0xFF, 0xFF, 0xFF)
CONTACT = (0x9A, 0x2C, 0x3F, 0xFF)


def rounded(x, y, left, top, right, bottom, radius):
    """Is (x, y) inside a rounded rectangle?"""
    if not (left <= x < right and top <= y < bottom):
        return False
    for cx, cy in (
        (left + radius, top + radius),
        (right - 1 - radius, top + radius),
        (left + radius, bottom - 1 - radius),
        (right - 1 - radius, bottom - 1 - radius),
    ):
        inside_x = (x < left + radius) or (x >= right - radius)
        inside_y = (y < top + radius) or (y >= bottom - radius)
        if inside_x and inside_y:
            near = abs(x - cx) <= radius and abs(y - cy) <= radius
            if near and (x - cx) ** 2 + (y - cy) ** 2 > radius * radius:
                return False
    return True


def pixel(x, y, size):
    """An SD card: a rounded body with one corner cut off, and contacts."""
    unit = size / 512.0
    if not rounded(x, y, 0, 0, size, size, int(96 * unit)):
        return (0, 0, 0, 0)

    left, top, right, bottom = (int(v * unit) for v in (136, 96, 376, 416))
    cut = int(72 * unit)
    if rounded(x, y, left, top, right, bottom, int(24 * unit)):
        # Bevel the top-right corner, which is what makes it read as a card.
        if x >= right - cut and y < top + cut and (right - x) + (y - top) < cut:
            return BACKGROUND
        # Contacts along the top of the card.
        pad_top, pad_bottom = top + int(40 * unit), top + int(128 * unit)
        if pad_top <= y < pad_bottom:
            column = int((x - left) / (30 * unit))
            if column in (1, 2, 3, 4, 5) and (x - left) % int(30 * unit) < int(18 * unit):
                return CONTACT
        return CARD
    return BACKGROUND


def png(size):
    rows = bytearray()
    for y in range(size):
        rows.append(0)  # filter type 0
        for x in range(size):
            rows.extend(pixel(x, y, size))

    def chunk(kind, payload):
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(rows), 9))
        + chunk(b"IEND", b"")
    )


def ico(png_bytes, size):
    """An ICO with a single PNG-compressed image, which Windows accepts."""
    header = struct.pack("<HHH", 0, 1, 1)
    entry = struct.pack(
        "<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png_bytes), 6 + 16
    )
    return header + entry + png_bytes


if __name__ == "__main__":
    for size, name in ((512, "icon.png"), (128, "128x128.png"), (32, "32x32.png")):
        with open(name, "wb") as handle:
            handle.write(png(size))
        print(f"wrote {name}")
    with open("icon.ico", "wb") as handle:
        handle.write(ico(png(256), 256))
    print("wrote icon.ico")
