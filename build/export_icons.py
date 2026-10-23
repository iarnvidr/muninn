#!/usr/bin/env python3
"""Export the approved photo raven to sidebar, PNG/ICO and PNG-backed ICNS sizes.

Requires ImageMagick. The source artwork is preserved; this only fits and exports
it into icon canvases. Generated assets are committed, so builds need no converter.
"""
from pathlib import Path
import struct
import subprocess
import tempfile

APP = Path(__file__).resolve().parents[1]


def magick(*args):
    subprocess.run(["magick", *map(str, args)], check=True)


def main():
    source = APP / "design/raven-photo-styled.png"
    icons = APP / "src-tauri/icons"
    with tempfile.TemporaryDirectory() as directory:
        tmp = Path(directory)
        mark = tmp / "raven.png"
        magick(source, "-trim", "+repage", "-resize", "800x840", mark)
        # A restrained dark tile gives the long red silhouette contrast in the Dock.
        magick("-size", "1024x1024", "xc:none", "-fill", "#101216",
               "-draw", "roundrectangle 32,32 992,992 190,190", mark,
               "-gravity", "center", "-composite", "PNG32:" + str(icons / "icon.png"))
        magick(source, "-trim", "+repage", "-resize", "128x128",
               "-gravity", "center", "-background", "none", "-extent", "128x128",
               "PNG32:" + str(APP / "ui/raven.png"))
        chunks = []
        for size, kind in [(16, b"icp4"), (32, b"icp5"), (64, b"icp6"),
                           (128, b"ic07"), (256, b"ic08"), (512, b"ic09"), (1024, b"ic10")]:
            path = tmp / f"{size}.png"
            magick(icons / "icon.png", "-resize", f"{size}x{size}", "PNG32:" + str(path))
            png = path.read_bytes()
            chunks.append(kind + struct.pack(">I", len(png) + 8) + png)
        payload = b"".join(chunks)
        (icons / "icon.icns").write_bytes(b"icns" + struct.pack(">I", len(payload) + 8) + payload)
        for size, name in [(32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png")]:
            magick(icons / "icon.png", "-resize", f"{size}x{size}", "PNG32:" + str(icons / name))
        magick(icons / "icon.png", "-define", "icon:auto-resize=256,128,64,48,32,16", icons / "icon.ico")


if __name__ == "__main__":
    main()
