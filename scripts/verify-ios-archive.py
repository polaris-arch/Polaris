#!/usr/bin/env python3
"""Final source admission, or explicit historical archive structure checks."""
import argparse
import importlib.util
import plistlib
import sys
import zipfile
from pathlib import Path

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('ios_libbox', Path(__file__).with_name('ios-libbox.py'))
libbox = importlib.util.module_from_spec(spec)
spec.loader.exec_module(libbox)


def verify_archive(ipa, historical=False):
    # Common source admission runs before opening or consuming the archive.
    # Structure checks cannot identify a statically linked Libbox source graph.
    libbox.inputs(historical=historical)
    with zipfile.ZipFile(ipa) as archive:
        names = archive.namelist()
        apps = [n for n in names if n.endswith(".app/Info.plist") and "/PlugIns/" not in n]
        assert len(apps) == 1, "Expected one host App"
        app_path = apps[0].removesuffix("Info.plist")
        app = plistlib.loads(archive.read(apps[0]))
        extensions = [n for n in names if n.startswith(app_path + "PlugIns/") and n.endswith(".appex/Info.plist")]
        assert len(extensions) == 1, "Expected one Packet Tunnel extension"
        extension = plistlib.loads(archive.read(extensions[0]))
        extension_path = extensions[0].removesuffix("Info.plist")
        assert extension["CFBundleIdentifier"] == app["CFBundleIdentifier"] + ".PacketTunnel", "Bundle IDs differ"
        assert app["PolarisAppGroup"] == extension["PolarisAppGroup"], "App Groups differ"
        assert app["PolarisAppGroup"].startswith("group.") and "$(" not in app["PolarisAppGroup"], "App Group setting was not resolved"
        assert app["UIDeviceFamily"] == [1, 2], "Host must support iPhone and iPad"
        assert extension["UIDeviceFamily"] == [1, 2], "Extension must support iPhone and iPad"
        assert extension["NSExtension"]["NSExtensionPointIdentifier"] == "com.apple.networkextension.packet-tunnel", "Wrong extension point"
        for prefix, info in [(app_path, app), (extension_path, extension)]:
            assert info["CFBundleSupportedPlatforms"] == ["iPhoneOS"], "Expected device build"
            assert tuple(map(int, info["MinimumOSVersion"].split("."))) >= (17, 0), "Unexpected deployment target"
            with archive.open(prefix + info["CFBundleExecutable"]) as binary:
                header = binary.read(8)
            assert header[:4] == b"\xcf\xfa\xed\xfe", "Expected a thin Mach-O 64-bit executable"
            assert int.from_bytes(header[4:8], "little") == 0x0100000C, "Expected arm64"
        stray = [n for n in names if n.startswith(app_path) and n.endswith(".a")]
        assert not stray, f"Build-only static libraries were copied into the App: {stray}"
        expected_geo = {p.name for p in (libbox.ROOT / "resources/data").glob("*.srs")}
        bundled_geo = {Path(n).name for n in names if n.startswith(app_path) and n.endswith(".srs")}
        assert expected_geo and expected_geo <= bundled_geo, "Bundled geo rules are missing"
        print(f"Historical structure-only: {app['CFBundleIdentifier']}: arm64, iOS 17+, iPhone/iPad, Packet Tunnel, shared group, {len(expected_geo)} geo rules; no stray .a resources")
        print("Structure-only history; final source/linkage, signing, permissions, lifecycle, memory and VPN traffic remain unverified.")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ipa", type=Path)
    parser.add_argument('--historical', action='store_true', help='Check historical archive structure only')
    args = parser.parse_args(argv)
    verify_archive(args.ipa, historical=args.historical)


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, KeyError, ValueError, AssertionError, zipfile.BadZipFile) as error:
        print(f'iOS archive failed: {error}', file=sys.stderr)
        sys.exit(1)
