"""Assemble native bundle metadata while preserving existing installation identity."""
import json
import os
import plistlib
import sys
from pathlib import Path


def metadata(config):
    info = {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": "muninn",
        "CFBundleIconFile": "icon",
        "CFBundleIdentifier": config["identifier"],
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": config["productName"],
        "CFBundleDisplayName": config["productName"],
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": config["version"],
        "CFBundleVersion": config["version"],
        "LSApplicationCategoryType": "public.app-category.productivity",
        "LSMinimumSystemVersion": config["bundle"]["macOS"]["minimumSystemVersion"],
        "NSHighResolutionCapable": True,
        "NSAppTransportSecurity": {"NSAllowsLocalNetworking": True},
    }
    # Both packaging paths and Tauri consume the same URL registration.
    extra = Path(__file__).resolve().parents[1] / "src-tauri" / config["bundle"]["macOS"]["infoPlist"]
    info.update(plistlib.loads(extra.read_bytes()))
    return info


def configured_bundle():
    root = Path(__file__).resolve().parents[1]
    config = json.loads((root / "src-tauri/tauri.conf.json").read_text())
    # Tauri's build-time TAURI_CONFIG is a merged JSON overlay. Use the same
    # overlay for the hand-assembled Info.plist or the compiled and packaged
    # identities would disagree (potentially sharing the user's data folder).
    override = json.loads(os.environ.get("TAURI_CONFIG", "{}"))
    if not isinstance(override, dict):
        raise ValueError("TAURI_CONFIG must be a JSON object")

    def merge(base, additions):
        for key, value in additions.items():
            if isinstance(value, dict) and isinstance(base.get(key), dict):
                merge(base[key], value)
            else:
                base[key] = value

    merge(config, override)
    return config


if __name__ == "__main__":
    config = configured_bundle()
    contents = Path(sys.argv[1]) / "Contents"
    (contents / "Info.plist").write_bytes(plistlib.dumps(metadata(config)))
    (contents / "PkgInfo").write_bytes(b"APPL????")
