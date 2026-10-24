#!/usr/bin/env python3
"""Pack a downloaded glTF model into one .glb for the viewer to embed.

    python3 scripts/pack-model.py assets/server_rack/scene.gltf viewer/assets/models/server_rack.glb

A model from Sketchfab comes as `scene.gltf` (JSON), `scene.bin` (the vertex
data) and a folder of textures, often 2048 px JPEGs. The viewer embeds each
model in the binary, so one file per model is easier, and a node a few dozen
pixels tall needs nothing like 2048 px: Bevy makes no mipmaps for PNG or JPEG,
so a big texture squeezed onto a small node shimmers as the camera moves.
This script makes a binary glTF (.glb) from the three: every texture shrunk to
at most --size px (ImageMagick's `magick`) and stored as PNG, since the
viewer's Bevy decodes PNG and not JPEG, and the vertex data and the PNGs in
one binary chunk. Geometry, materials, node transforms and animations are
copied as they are; the viewer does the rest when it loads the file.

A .glb is a 12-byte header and two chunks, each an 8-byte header (length,
type) and its data padded to 4 bytes: the JSON, then the binary data. A
buffer without a `uri` is that binary chunk, and an image may point at a
bufferView of it instead of a file.
"""

import argparse
import json
import struct
import subprocess
from pathlib import Path

GLB_MAGIC = b"glTF"
GLB_VERSION = 2
CHUNK_JSON = 0x4E4F534A  # "JSON", little-endian
CHUNK_BIN = 0x004E4942  # "BIN\0"


def pad(data: bytes, to: int, fill: bytes) -> bytes:
    return data + fill * (-len(data) % to)


def shrink_to_png(path: Path, size: int) -> bytes:
    # `>` only ever shrinks; png24 keeps three 8-bit channels (ImageMagick
    # would otherwise pick grey or a palette for some textures); -strip drops
    # the colour-profile chunks, so a normal map's values pass through as data.
    out = subprocess.run(
        ["magick", str(path), "-resize", f"{size}x{size}>", "-strip", "png24:-"],
        check=True,
        capture_output=True,
    )
    return out.stdout


def pack(source: Path, size: int) -> bytes:
    gltf = json.loads(source.read_text())
    folder = source.parent
    if len(gltf.get("buffers", [])) != 1:
        raise SystemExit(f"{source}: expected one buffer, found {len(gltf.get('buffers', []))}")
    blob = (folder / gltf["buffers"][0]["uri"]).read_bytes()

    # Each texture goes after the vertex data, 4-byte aligned, in a
    # bufferView of its own.
    views = gltf.setdefault("bufferViews", [])
    for image in gltf.get("images", []):
        png = shrink_to_png(folder / image.pop("uri"), size)
        blob = pad(blob, 4, b"\0")
        views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(png)})
        blob += png
        image["bufferView"] = len(views) - 1
        image["mimeType"] = "image/png"

    blob = pad(blob, 4, b"\0")
    gltf["buffers"] = [{"byteLength": len(blob)}]
    text = pad(json.dumps(gltf, separators=(",", ":")).encode(), 4, b" ")

    total = 12 + 8 + len(text) + 8 + len(blob)
    return b"".join(
        [
            GLB_MAGIC + struct.pack("<II", GLB_VERSION, total),
            struct.pack("<II", len(text), CHUNK_JSON) + text,
            struct.pack("<II", len(blob), CHUNK_BIN) + blob,
        ]
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("source", type=Path, help="the .gltf file")
    parser.add_argument("output", type=Path, help="the .glb to write")
    parser.add_argument("--size", type=int, default=512, help="largest texture side, px (512)")
    args = parser.parse_args()
    glb = pack(args.source, args.size)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(glb)
    print(f"{args.output}: {len(glb) / 1024:.0f} KiB")


if __name__ == "__main__":
    main()
