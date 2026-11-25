#!/usr/bin/env python3
import math
import os
import struct
import zlib


def save_png(path: str, w: int, h: int, pixels: bytearray) -> None:
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        raw.extend(pixels[y * w * 4 : (y + 1) * w * 4])

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    with open(path, "wb") as f:
        f.write(
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(bytes(raw)))
            + chunk(b"IEND", b"")
        )


def main() -> None:
    os.makedirs("assets", exist_ok=True)
    w = h = 144

    # 1. key.png: sfondo nero puro (144x144) per la massima leggibilità dei numeri
    black_pixels = bytearray(bytes((0, 0, 0, 255)) * (w * h))
    save_png("assets/key.png", w, h, black_pixels)

    # 2. icon.png: tachimetro (144x144) per lo store e la barra laterale
    px = bytearray(w * h * 4)
    cx, cy = w / 2.0, h / 2.0 + 6.0

    for y in range(h):
        for x in range(w):
            idx = (y * w + x) * 4
            dx_box = max(abs(x - w / 2.0) - 54.0, 0.0)
            dy_box = max(abs(y - h / 2.0) - 54.0, 0.0)
            if math.hypot(dx_box, dy_box) <= 18.0:
                r, g, b, a = 20, 24, 32, 255
            else:
                r, g, b, a = 0, 0, 0, 0

            dx, dy = x - cx, y - cy
            dist = math.hypot(dx, dy)
            ang = (math.degrees(math.atan2(dy, dx)) + 360.0) % 360.0
            in_arc = (ang >= 145.0) or (ang <= 35.0)

            if 38.0 <= dist <= 49.0 and in_arc:
                t = ((ang - 145.0) % 360.0) / 250.0
                r = int(34 * (1 - t) + 56 * t)
                g = int(197 * (1 - t) + 189 * t)
                b = int(94 * (1 - t) + 248 * t)
                a = 255

            rad = math.radians(-40.0)
            nx, ny = math.cos(rad), math.sin(rad)
            proj = dx * nx + dy * ny
            perp = abs(dx * (-ny) + dy * nx)
            if 0.0 <= proj <= 35.0 and perp <= 2.6:
                r, g, b, a = 248, 250, 252, 255

            if dist <= 6.5:
                r, g, b, a = 56, 189, 248, 255

            px[idx : idx + 4] = bytes((r, g, b, a))

    save_png("assets/icon.png", w, h, px)
    print("Generated assets/icon.png and assets/key.png")


if __name__ == "__main__":
    main()
