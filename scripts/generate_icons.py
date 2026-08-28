#!/usr/bin/env python3
import os
import struct
import sys
import zlib
from collections import deque


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
SOURCE_CANDIDATES = [
    os.path.join(ROOT, "logo.png"),
    os.path.join(ROOT, "assets", "logo.png"),
]


def resolve_source():
    for candidate in SOURCE_CANDIDATES:
        if os.path.isfile(candidate):
            return candidate
    return SOURCE_CANDIDATES[0]


def read_png(path):
    with open(path, "rb") as f:
        if f.read(8) != b"\x89PNG\r\n\x1a\n":
            raise ValueError("not a png")
        width = height = bit_depth = color_type = None
        data = bytearray()
        while True:
            raw = f.read(8)
            if not raw:
                break
            length, chunk_type = struct.unpack(">I4s", raw)
            chunk = f.read(length)
            f.read(4)
            if chunk_type == b"IHDR":
                width, height, bit_depth, color_type, compression, filter_method, interlace = struct.unpack(">IIBBBBB", chunk)
                if bit_depth != 8 or color_type not in (2, 6) or compression != 0 or filter_method != 0 or interlace != 0:
                    raise ValueError("only 8-bit RGB/RGBA non-interlaced PNG is supported")
            elif chunk_type == b"IDAT":
                data.extend(chunk)
            elif chunk_type == b"IEND":
                break

    channels = 4 if color_type == 6 else 3
    stride = width * channels
    decoded = zlib.decompress(bytes(data))
    rows = []
    prev = [0] * stride
    pos = 0
    bpp = channels
    for _ in range(height):
        filter_type = decoded[pos]
        pos += 1
        row = list(decoded[pos:pos + stride])
        pos += stride
        for i, value in enumerate(row):
            left = row[i - bpp] if i >= bpp else 0
            up = prev[i]
            up_left = prev[i - bpp] if i >= bpp else 0
            if filter_type == 1:
                row[i] = (value + left) & 255
            elif filter_type == 2:
                row[i] = (value + up) & 255
            elif filter_type == 3:
                row[i] = (value + ((left + up) // 2)) & 255
            elif filter_type == 4:
                p = left + up - up_left
                pa = abs(p - left)
                pb = abs(p - up)
                pc = abs(p - up_left)
                pr = left if pa <= pb and pa <= pc else up if pb <= pc else up_left
                row[i] = (value + pr) & 255
            elif filter_type != 0:
                raise ValueError(f"unsupported PNG filter {filter_type}")
        prev = row
        rows.append(row)

    rgba = []
    for row in rows:
        out = []
        for x in range(width):
            idx = x * channels
            if channels == 4:
                out.extend(row[idx:idx + 4])
            else:
                out.extend([row[idx], row[idx + 1], row[idx + 2], 255])
        rgba.append(out)
    return width, height, rgba


def write_png(path, width, height, rows):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    raw = bytearray()
    for row in rows:
        raw.append(0)
        raw.extend(row)
    chunks = []
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    chunks.append((b"IHDR", ihdr))
    chunks.append((b"IDAT", zlib.compress(bytes(raw), 9)))
    chunks.append((b"IEND", b""))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        for chunk_type, chunk in chunks:
            f.write(struct.pack(">I", len(chunk)))
            f.write(chunk_type)
            f.write(chunk)
            f.write(struct.pack(">I", zlib.crc32(chunk_type + chunk) & 0xFFFFFFFF))


def is_light_background(pixel):
    r, g, b, _ = pixel
    avg = (r + g + b) / 3
    return avg >= 218 and max(r, g, b) - min(r, g, b) <= 42


def remove_connected_background(width, height, rows):
    pixels = [
        [rows[y][x * 4:x * 4 + 4] for x in range(width)]
        for y in range(height)
    ]
    seen = [[False] * width for _ in range(height)]
    q = deque()
    for x in range(width):
        q.append((x, 0))
        q.append((x, height - 1))
    for y in range(height):
        q.append((0, y))
        q.append((width - 1, y))

    while q:
        x, y = q.popleft()
        if x < 0 or y < 0 or x >= width or y >= height or seen[y][x]:
            continue
        seen[y][x] = True
        if not is_light_background(pixels[y][x]):
            continue
        pixels[y][x][3] = 0
        q.append((x + 1, y))
        q.append((x - 1, y))
        q.append((x, y + 1))
        q.append((x, y - 1))

    return [
        [channel for pixel in row for channel in pixel]
        for row in pixels
    ]


def crop_to_alpha(width, height, rows, padding=0):
    min_x, min_y = width, height
    max_x, max_y = -1, -1
    for y in range(height):
        row = rows[y]
        for x in range(width):
            if row[x * 4 + 3] > 0:
                min_x = min(min_x, x)
                min_y = min(min_y, y)
                max_x = max(max_x, x)
                max_y = max(max_y, y)
    if max_x < min_x or max_y < min_y:
        raise ValueError("icon became empty after background removal")

    min_x = max(0, min_x - padding)
    min_y = max(0, min_y - padding)
    max_x = min(width - 1, max_x + padding)
    max_y = min(height - 1, max_y + padding)
    cropped = []
    for y in range(min_y, max_y + 1):
        start = min_x * 4
        end = (max_x + 1) * 4
        cropped.append(rows[y][start:end])
    return max_x - min_x + 1, max_y - min_y + 1, cropped


def sample(rows, width, height, x, y):
    x = min(max(x, 0), width - 1)
    y = min(max(y, 0), height - 1)
    idx = (y * width + x) * 4
    flat = [channel for row in rows for channel in row]
    return flat[idx:idx + 4]


def resize_square(width, height, rows, size):
    src = [channel for row in rows for channel in row]
    out = []
    scale = max(width, height)
    offset_x = (scale - width) / 2
    offset_y = (scale - height) / 2
    for y in range(size):
        row = []
        sy = ((y + 0.5) * scale / size) - 0.5 - offset_y
        y0 = int(sy)
        y1 = y0 + 1
        wy = sy - y0
        for x in range(size):
            sx = ((x + 0.5) * scale / size) - 0.5 - offset_x
            x0 = int(sx)
            x1 = x0 + 1
            wx = sx - x0
            accum = [0.0, 0.0, 0.0, 0.0]
            for px, fx in ((x0, 1 - wx), (x1, wx)):
                for py, fy in ((y0, 1 - wy), (y1, wy)):
                    weight = fx * fy
                    if px < 0 or py < 0 or px >= width or py >= height:
                        pixel = [0, 0, 0, 0]
                    else:
                        idx = (py * width + px) * 4
                        pixel = src[idx:idx + 4]
                    for i in range(4):
                        accum[i] += pixel[i] * weight
            row.extend([max(0, min(255, int(round(v)))) for v in accum])
        out.append(row)
    return out


def main():
    source = sys.argv[1] if len(sys.argv) > 1 else resolve_source()
    if not os.path.isfile(source):
        raise FileNotFoundError(f"source icon not found: {source}")
    width, height, rows = read_png(source)
    rows = remove_connected_background(width, height, rows)
    width, height, rows = crop_to_alpha(width, height, rows)

    targets = {
        "assets-clean-icon.png": 1024,
        "frontend/public/icon.png": 256,
        "backend/ui/icon.png": 256,
        "ui/icon.png": 256,
        "build_fpk/ICON.PNG": 64,
        "build_fpk/ICON_256.PNG": 256,
        "build_fpk/app/www/icon.png": 256,
    }
    for size in (16, 24, 32, 48, 64, 72, 96, 128, 256):
        targets[f"build_fpk/app/ui/images/icon_{size}.png"] = size
        targets[f"build_fpk/app/ui/images/rmux_{size}.png"] = size
    targets["build_fpk/app/ui/images/icon_{0}.png"] = 256
    targets["build_fpk/app/ui/images/rmux.png"] = 256

    for rel, size in targets.items():
        path = os.path.join(ROOT, rel)
        resized = resize_square(width, height, rows, size)
        write_png(path, size, size, resized)
        print(f"generated {rel}")


if __name__ == "__main__":
    main()
