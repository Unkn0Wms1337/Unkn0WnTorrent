"""РРєРѕРЅРєР° Unkn0WnTorrent РёР· РєР°СЂС‚РёРЅРєРё РїРѕР»СЊР·РѕРІР°С‚РµР»СЏ (assets/icon_source.png).

РќРёС‡РµРіРѕ РЅРµ РїРµСЂРµСЂРёСЃРѕРІС‹РІР°РµРј: Р±РµСЂС‘Рј РёСЃС…РѕРґРЅРёРє 128x128, РјР°СЃС€С‚Р°Р±РёСЂСѓРµРј Рё Р»РёС€СЊ
СЃР»РµРіРєР° СЃРєСЂСѓРіР»СЏРµРј СѓРіР»С‹ РїР»РёС‚РєРё. Р‘РµР· РІРЅРµС€РЅРёС… Р·Р°РІРёСЃРёРјРѕСЃС‚РµР№ (СЃРІРѕР№ PNG-РґРµРєРѕРґРµСЂ).
"""
import math
import os
import struct
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, "assets")
SOURCE = os.path.join(ASSETS, "icon_source.png")

CORNER_RADIUS = 0.115  # РґРѕР»СЏ РѕС‚ СЃС‚РѕСЂРѕРЅС‹ РїР»РёС‚РєРё


# ------------------------------------------------------------- png decode
def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def _unfilter(data, w, h, bpp):
    stride = w * bpp
    out = bytearray(stride * h)
    pos = 0
    prev = bytearray(stride)
    for y in range(h):
        ft = data[pos]
        pos += 1
        line = bytearray(data[pos:pos + stride])
        pos += stride
        if ft == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif ft == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ft == 3:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif ft == 4:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                c = prev[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + _paeth(a, prev[i], c)) & 0xFF
        elif ft != 0:
            raise ValueError("bad filter %d" % ft)
        out[y * stride:(y + 1) * stride] = line
        prev = line
    return out


def read_png(path):
    with open(path, "rb") as f:
        blob = f.read()
    if blob[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a png")
    pos = 8
    idat = bytearray()
    w = h = depth = ctype = None
    while pos < len(blob):
        (length,) = struct.unpack(">I", blob[pos:pos + 4])
        tag = blob[pos + 4:pos + 8]
        body = blob[pos + 8:pos + 8 + length]
        pos += 12 + length
        if tag == b"IHDR":
            w, h, depth, ctype, comp, filt, inter = struct.unpack(">IIBBBBB", body)
            if depth != 8 or ctype not in (2, 6) or inter:
                raise ValueError("unsupported png (depth=%d type=%d)" % (depth, ctype))
        elif tag == b"IDAT":
            idat += body
        elif tag == b"IEND":
            break
    bpp = 4 if ctype == 6 else 3
    raw = _unfilter(zlib.decompress(bytes(idat)), w, h, bpp)
    rgba = bytearray(w * h * 4)
    for i in range(w * h):
        s = i * bpp
        d = i * 4
        if bpp == 4:
            rgba[d:d + 4] = raw[s:s + 4]
        else:
            rgba[d] = raw[s]
            rgba[d + 1] = raw[s + 1]
            rgba[d + 2] = raw[s + 2]
            rgba[d + 3] = 255
    return w, h, rgba


# ------------------------------------------------------------- resample
def sample(src, sw, sh, x, y):
    """Р‘РёР»РёРЅРµР№РЅР°СЏ РІС‹Р±РѕСЂРєР° RGB СЃ РєР»Р°РјРїРѕРј РїРѕ РєСЂР°СЏРј (Р°Р»СЊС„Р° РёСЃС…РѕРґРЅРёРєР° РЅРµ РЅСѓР¶РЅР°:
    РїР»РёС‚РєР° С‡С‘СЂРЅР°СЏ Рё РЅРµРїСЂРѕР·СЂР°С‡РЅР°СЏ, СЃРєСЂСѓРіР»РµРЅРёРµ РґРµР»Р°РµРј СЃР°РјРё)."""
    x = min(max(x, 0.0), sw - 1.0)
    y = min(max(y, 0.0), sh - 1.0)
    x0 = int(x)
    y0 = int(y)
    x1 = min(x0 + 1, sw - 1)
    y1 = min(y0 + 1, sh - 1)
    fx = x - x0
    fy = y - y0
    out = [0.0, 0.0, 0.0]
    for (cx, cy, wgt) in (
        (x0, y0, (1 - fx) * (1 - fy)),
        (x1, y0, fx * (1 - fy)),
        (x0, y1, (1 - fx) * fy),
        (x1, y1, fx * fy),
    ):
        o = (cy * sw + cx) * 4
        out[0] += src[o] * wgt
        out[1] += src[o + 1] * wgt
        out[2] += src[o + 2] * wgt
    return out[0], out[1], out[2]


def corner_coverage(n, px, py, step, radius):
    """Р”РѕР»СЏ РїРѕРґРІС‹Р±РѕСЂРѕРє РїРёРєСЃРµР»СЏ РІРЅСѓС‚СЂРё СЃРєСЂСѓРіР»С‘РЅРЅРѕРіРѕ РїСЂСЏРјРѕСѓРіРѕР»СЊРЅРёРєР°."""
    inside = 0
    total = 0
    for j in range(step):
        sy = py + (j + 0.5) / step
        for i in range(step):
            sx = px + (i + 0.5) / step
            cx = min(max(sx, radius), n - radius)
            cy = min(max(sy, radius), n - radius)
            dx, dy = sx - cx, sy - cy
            total += 1
            if dx * dx + dy * dy <= radius * radius:
                inside += 1
    return inside / total


def cubic_w(t):
    """Catmull-Rom."""
    t = abs(t)
    if t < 1.0:
        return 1.5 * t ** 3 - 2.5 * t ** 2 + 1.0
    if t < 2.0:
        return -0.5 * t ** 3 + 2.5 * t ** 2 - 4.0 * t + 2.0
    return 0.0


def sample_cubic(src, sw, sh, x, y):
    """Бикубическая (Catmull-Rom) выборка — для увеличения, чтобы знак не мылился."""
    xi = int(math.floor(x))
    yi = int(math.floor(y))
    out = [0.0, 0.0, 0.0]
    for dy in range(-1, 3):
        sy = min(max(yi + dy, 0), sh - 1)
        wy = cubic_w(y - (yi + dy))
        if wy == 0.0:
            continue
        for dx in range(-1, 3):
            sx = min(max(xi + dx, 0), sw - 1)
            wgt = wy * cubic_w(x - (xi + dx))
            if wgt == 0.0:
                continue
            o = (sy * sw + sx) * 4
            out[0] += src[o] * wgt
            out[1] += src[o + 1] * wgt
            out[2] += src[o + 2] * wgt
    return out[0], out[1], out[2]


def render(src, sw, sh, n, radius):
    step = 4 if n < 128 else 3
    ss = 6 if n <= 24 else (4 if n < 128 else 3)
    upscale = n > sw
    rgba = bytearray(n * n * 4)
    scale = sw / n
    for y in range(n):
        for x in range(n):
            r = g = b = 0.0
            for j in range(ss):
                sy = (y + (j + 0.5) / ss) * scale
                for i in range(ss):
                    sx = (x + (i + 0.5) / ss) * scale
                    if upscale:
                        cr, cg, cb = sample_cubic(src, sw, sh, sx - 0.5, sy - 0.5)
                    else:
                        cr, cg, cb = sample(src, sw, sh, sx - 0.5, sy - 0.5)
                    r += cr
                    g += cg
                    b += cb
            cnt = ss * ss
            cov = corner_coverage(n, x, y, step, radius)
            if cov <= 0.0:
                continue
            o = (y * n + x) * 4
            rgba[o] = int(min(255.0, max(0.0, r / cnt)) + 0.5)
            rgba[o + 1] = int(min(255.0, max(0.0, g / cnt)) + 0.5)
            rgba[o + 2] = int(min(255.0, max(0.0, b / cnt)) + 0.5)
            rgba[o + 3] = int(min(255.0, cov * 255.0) + 0.5)
    return bytes(rgba)


# ---------------------------------------------------------------- files
def write_png(path, s, rgba):
    raw = bytearray()
    for y in range(s):
        raw.append(0)
        raw += rgba[y * s * 4:(y + 1) * s * 4]

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", s, s, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def dib_entry(s, rgba):
    header = struct.pack(
        "<IiiHHIIiiII", 40, s, s * 2, 1, 32, 0, s * s * 4, 0, 0, 0, 0
    )
    pixels = bytearray()
    for y in range(s - 1, -1, -1):
        for x in range(s):
            r, g, b, a = rgba[(y * s + x) * 4:(y * s + x) * 4 + 4]
            pixels += bytes((b, g, r, a))
    mask_row = ((s + 31) // 32) * 4
    mask = bytearray(mask_row * s)
    for y in range(s):
        for x in range(s):
            if rgba[(y * s + x) * 4 + 3] == 0:
                mask[y * mask_row + x // 8] |= 0x80 >> (x % 8)
    return header + bytes(pixels) + bytes(mask)


def write_ico(path, entries):
    count = len(entries)
    offset = 6 + 16 * count
    header = struct.pack("<HHH", 0, 1, count)
    directory = bytearray()
    blobs = bytearray()
    for s, dib in entries:
        directory += struct.pack(
            "<BBBBHHII", 0 if s >= 256 else s, 0 if s >= 256 else s, 0, 0, 1, 32,
            len(dib), offset,
        )
        blobs += dib
        offset += len(dib)
    with open(path, "wb") as f:
        f.write(header + directory + blobs)


def main():
    sw, sh, src = read_png(SOURCE)
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    cache = {}
    for s in sizes:
        rgba = render(src, sw, sh, s, max(2.0, CORNER_RADIUS * s))
        cache[s] = rgba
        write_png(os.path.join(ASSETS, "icon_%d.png" % s), s, rgba)
    write_png(os.path.join(ASSETS, "icon_preview.png"), 256, cache[256])
    with open(os.path.join(ASSETS, "icon_64.rgba"), "wb") as f:
        f.write(cache[64])
    write_ico(
        os.path.join(ASSETS, "icon.ico"),
        [(s, dib_entry(s, cache[s])) for s in sizes],
    )
    print("ok", os.path.join(ASSETS, "icon.ico"))


if __name__ == "__main__":
    main()

