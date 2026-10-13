#!/usr/bin/env python3
"""Frozen formal Xcode Cargo hook; no CLI options server, signing or device action."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import sys

sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from artifacts import file_pin, module, read_receipt
from consume import app_identity, verify_link
from fork_input import ROOT, canonical, load, require, sha
from producer import ACTIVE_FDS, private_dir, run, write_json
from budget import reservation


def target_for(platform, arches, configuration):
    require(configuration == 'release', 'formal App hook requires release configuration')
    require(len(arches.split()) == 1, 'one actual Apple architecture required')
    targets = {('iphoneos', 'arm64'): 'aarch64-apple-ios',
               ('iphonesimulator', 'arm64'): 'aarch64-apple-ios-sim',
               ('iphonesimulator', 'x86_64'): 'x86_64-apple-ios'}
    require((platform, arches) in targets, 'unsupported actual App platform/architecture')
    return targets[(platform, arches)]


def selected_wire_bytes(raw, triple):
    """Inspect every fat entry, then select the actual Cargo architecture/platform."""
    specs = {'aarch64-apple-ios': (0x100000c, 2),
             'aarch64-apple-ios-sim': (0x100000c, 7), 'x86_64-apple-ios': (0x1000007, 7)}
    require(triple in specs and len(raw) <= 1024 ** 3, 'bounded selected wire target required')
    parser = module('apple_selected_wire_carrier', 'scripts/apple-carrier.py')
    helper = module('apple_selected_wire_info', 'scripts/libbox-patches/android-source.py')
    observed = parser.container(raw, helper)
    require(observed['kind'] in ('bsd-ar', 'fat-ar'), 'actual static wire input required')
    slices = observed['slices'] if observed['kind'] == 'fat-ar' else [
        {'offset': 0, 'bytes': len(raw), 'archive': observed}]
    cpu, platform = specs[triple]
    matching = []
    for row in slices:
        code = [member['machO'] for member in row['archive']['members'] if member['classification'] == 'code']
        require(code and len({obj['cpu'] for obj in code}) == 1, 'mixed or missing wire architecture')
        require('cpu' not in row or (row['cpu'], row['subCPU']) == (code[0]['cpu'], code[0]['subCPU']),
                'fat header/payload architecture differs')
        require(len({(obj['cpu'], obj['subCPU']) for obj in code}) == 1, 'mixed wire CPU/subtype')
        platforms = [p['platform'] for obj in code for p in obj['platformCommands']]
        require(platforms and len(set(platforms)) == 1, 'mixed or missing wire platform')
        if code[0]['cpu'] == cpu and platforms[0] == platform:
            matching.append(row)
    require(len(matching) == 1, 'exact Cargo architecture/platform wire slice required')
    selected = matching[0]
    thin = raw[selected['offset']:selected['offset'] + selected['bytes']]
    require(thin.startswith(b'!<arch>\n'), 'selected wire slice is not a static archive')
    return thin, {'target': triple, 'cpu': cpu, 'platform': platform,
                  'offset': selected['offset'], 'bytes': selected['bytes'], 'container': observed['kind']}


def verify_start_app(archive, log):
    # This predicate needs a defined native text symbol, not Rust's LLVM IR.
    # The final Xcode link still validates the complete unchanged archive.
    symbols = run(['xcrun', 'nm', '--no-llvm-bc', '-gU', archive], log=log)
    require(re.search(r'\b[Tt]\s+_start_app\s*$', symbols, re.MULTILINE),
            'actual Rust start_app definition missing')


def build():
    root = private_dir(Path(os.environ['POLARIS_APP_WORK']))
    require(os.environ.get('POLARIS_BUILD_RESERVATION_FD') is not None,
            'Rust compile must inherit the admitted App reservation before Xcode')
    with reservation(root, load(), ACTIVE_FDS):
        build_reserved()


def build_reserved():
    require(os.uname().sysname == 'Darwin' and os.environ.get('POLARIS_FORMAL_APP_BUILD') == '1',
            'formal Apple hook must be called by the admitted App builder')
    value = load()
    triple = target_for(os.environ['PLATFORM_NAME'], os.environ['ARCHS'], os.environ['CONFIGURATION'])
    root = private_dir(Path(os.environ['POLARIS_APP_WORK']))
    target = private_dir(Path(os.environ['POLARIS_APP_RUST_TARGET']))
    require(target.stat().st_dev == root.stat().st_dev, 'App Rust target must share the reserved filesystem')
    scratch = private_dir(root / 'rust-link-check')
    link = verify_link(Path(os.environ['POLARIS_MOBILE_LINK_INPUT']),
                       os.environ['POLARIS_MOBILE_LINK_INPUT_SHA256'], scratch)
    identity = app_identity(status_log=root / 'original-source-status-before-rust.log')
    require(link['App'] == identity, 'formal Rust source differs from frozen App input')
    sdk = Path(run(['xcrun', '--sdk', os.environ['PLATFORM_NAME'], '--show-sdk-path']).strip()).resolve(strict=True)
    require(Path(os.environ['SDKROOT']).resolve(strict=True) == sdk, 'Xcode SDKROOT differs from selected SDK')
    sdk_version = value['apple']['iPhoneSDK' if os.environ['PLATFORM_NAME'] == 'iphoneos' else 'simulatorSDK']
    require(run(['xcrun', '--sdk', os.environ['PLATFORM_NAME'], '--show-sdk-version']).strip() == sdk_version,
            'actual App SDK version differs')
    require(run(['xcodebuild', '-version']).strip() == 'Xcode ' + value['apple']['xcodeVersion'] +
            '\nBuild version ' + value['apple']['xcodeBuild'], 'actual Xcode version/build differs')
    mac_sdk = run(['xcrun', '--sdk', 'macosx', '--show-sdk-path']).strip()
    # Tauri's build/codegen loads the target JSON; supply the platform merge just
    # as ios build does, without mutating tracked Cargo/PBX/Info files.
    ios_config = json.loads((ROOT / 'src-tauri/tauri.ios.conf.json').read_bytes())
    env = dict(os.environ, CARGO_TARGET_DIR=str(target), IPHONEOS_DEPLOYMENT_TARGET='17.0',
               TAURI_CONFIG=json.dumps(ios_config), TAURI_ENV_TARGET_TRIPLE=triple,
               TAURI_ENV_ARCH=triple.split('-')[0], TAURI_ENV_PLATFORM='ios', TAURI_ENV_FAMILY='unix',
               TAURI_IOS_PROJECT_PATH=str(ROOT / 'src-tauri/gen/apple'))
    suffix = triple.replace('-', '_')
    slice_name = ('ios-arm64' if os.environ['PLATFORM_NAME'] == 'iphoneos' else
                  'ios-arm64-simulator' if os.environ['ARCHS'] == 'arm64' else 'ios-x86_64-simulator')
    selected = link['componentOutput']['slices'][slice_name]
    library = Path(link['framework']) / selected['libraryIdentifier'] / 'Libbox.framework/Libbox'
    library = library.resolve(strict=True)
    require(file_pin(library) == selected['binary'], 'actual selected wire archive differs')
    thin, selection = selected_wire_bytes(library.read_bytes(), triple)
    wire = root / ('wire-' + triple + '.a')
    require(not wire.exists() and not wire.is_symlink(), 'actual selected wire output already retained')
    with wire.open('xb') as stream:
        stream.write(thin)
    require(file_pin(library) == selected['binary'], 'original wire archive changed during selection')
    env['POLARIS_APPLE_WIRE_ARCHIVE'] = str(wire)
    for kind in ('CFLAGS', 'CXXFLAGS'):
        env[kind + '_' + suffix] = '-isysroot ' + str(sdk)
        for host in ('aarch64_apple_darwin', 'x86_64_apple_darwin'):
            env[kind + '_' + host] = '-isysroot ' + mac_sdk
    env['OBJC_INCLUDE_PATH_' + suffix] = str(sdk / 'usr/include')
    tool_paths = {name: Path(run(['rustup', 'which', name]).strip()).resolve(strict=True)
                  for name in ('cargo', 'rustc')}
    tool_pins = {name: {'path': str(path), **file_pin(path)} for name, path in tool_paths.items()}
    env['RUSTC'] = str(tool_paths['rustc'])
    # Swift bridge symbols resolve in the final Xcode App link, after this archive.
    command = [str(tool_paths['cargo']), 'rustc', '--locked', '--release', '--lib', '--package', 'polaris',
               '--target', triple, '--features', 'tauri/custom-protocol', '--crate-type', 'staticlib']
    run(command, cwd=ROOT / 'src-tauri', env=env, log=root / ('original-rust-' + triple + '.log'))
    archive = target / triple / 'release/libpolaris_lib.a'
    require(not archive.is_symlink() and archive.read_bytes().startswith(b'!<arch>\n'), 'actual Rust static archive required')
    verify_start_app(archive, root / ('original-rust-symbols-' + triple + '.log'))
    pin = file_pin(archive)
    dest = ROOT / 'src-tauri/gen/apple/Externals' / os.environ['ARCHS'] / 'release/libapp.a'
    for parent in [dest, *dest.parents]:
        require(not parent.is_symlink(), 'Rust App link output redirected')
        if parent == ROOT:
            break
    dest.parent.mkdir(parents=True, exist_ok=True)
    stage = dest.with_name('libapp.a.formal-stage')
    require(not stage.exists(), 'unaccepted Rust link stage retained')
    shutil.copyfile(archive, stage)
    require(file_pin(stage) == pin, 'Rust static archive copy differs')
    os.replace(stage, dest)
    require(file_pin(dest) == pin and
            app_identity(status_log=root / 'original-source-status-after-rust.log') == identity,
            'Rust link bytes/source changed')
    receipt = {'schema': 'polaris-formal-apple-rust-v1', 'App': identity, 'target': triple,
               'sdk': str(sdk), 'sdkVersion': sdk_version, 'command': command,
               'rustc': run([tool_paths['rustc'], '-vV']), 'tools': tool_pins,
               'cargoLock': file_pin(ROOT / 'Cargo.lock'),
               'iosConfig': file_pin(ROOT / 'src-tauri/tauri.ios.conf.json'),
               'linkInputSha256': os.environ['POLARIS_MOBILE_LINK_INPUT_SHA256'],
               'originalWireArchive': {'path': str(library), **file_pin(library)},
               'wireSelection': selection, 'wireArchive': {'path': str(wire), **file_pin(wire)},
               'archive': pin, 'linkedArchive': str(dest), 'devicePassed': False}
    receipt['fingerprint'] = sha(canonical(receipt))
    require(file_pin(wire) == {'bytes': len(thin), 'sha256': sha(thin)} and
            file_pin(library) == selected['binary'], 'wire input changed during Rust compile')
    require(all(file_pin(pin['path']) == {key: pin[key] for key in ('bytes', 'sha256')}
                for pin in tool_pins.values()), 'actual Cargo/Rust compiler changed during build')
    output = root / ('rust-' + triple + '.json')
    require(not output.exists(), 'actual Rust receipt already frozen')
    write_json(output, receipt)


def verify():
    # Xcode's build service is not an fd transport. This phase never launches
    # Cargo: the builder has already compiled under its inherited reservation.
    # Metadata readers must not attempt to reuse a descriptor closed by Xcode.
    os.environ.pop('POLARIS_BUILD_RESERVATION_FD', None)
    require(os.environ.get('POLARIS_FORMAL_APP_BUILD') == '1', 'formal App verification entry required')
    root = private_dir(Path(os.environ['POLARIS_APP_WORK']))
    triple = target_for(os.environ['PLATFORM_NAME'], os.environ['ARCHS'], os.environ['CONFIGURATION'])
    link = verify_link(Path(os.environ['POLARIS_MOBILE_LINK_INPUT']),
                       os.environ['POLARIS_MOBILE_LINK_INPUT_SHA256'], private_dir(root / 'rust-link-check'))
    receipt = read_receipt(root / ('rust-' + triple + '.json'))
    expected = ROOT / 'src-tauri/gen/apple/Externals' / os.environ['ARCHS'] / 'release/libapp.a'
    require(receipt['schema'] == 'polaris-formal-apple-rust-v1' and receipt['target'] == triple and
            receipt['fingerprint'] == sha(canonical({k: v for k, v in receipt.items() if k != 'fingerprint'})) and
            receipt['App'] == link['App'] and receipt['linkInputSha256'] == os.environ['POLARIS_MOBILE_LINK_INPUT_SHA256'] and
            Path(receipt['linkedArchive']) == expected and file_pin(expected) == receipt['archive'] and
            receipt['cargoLock'] == file_pin(ROOT / 'Cargo.lock') and
            receipt['iosConfig'] == file_pin(ROOT / 'src-tauri/tauri.ios.conf.json') and
            Path(os.environ['SDKROOT']).resolve(strict=True) == Path(receipt['sdk']) and
            receipt['devicePassed'] is False, 'actual compiled Rust/Xcode input differs')
    slice_name = ('ios-arm64' if os.environ['PLATFORM_NAME'] == 'iphoneos' else
                  'ios-arm64-simulator' if os.environ['ARCHS'] == 'arm64' else 'ios-x86_64-simulator')
    selected = link['componentOutput']['slices'][slice_name]
    original = (Path(link['framework']) / selected['libraryIdentifier'] / 'Libbox.framework/Libbox').resolve(strict=True)
    thin, selection = selected_wire_bytes(original.read_bytes(), triple)
    wire = root / ('wire-' + triple + '.a')
    require(receipt['originalWireArchive'] == {'path': str(original), **selected['binary']} and
            file_pin(original) == selected['binary'] and receipt['wireSelection'] == selection and
            receipt['wireArchive'] == {'path': str(wire), 'bytes': len(thin), 'sha256': sha(thin)} and
            file_pin(wire) == {'bytes': len(thin), 'sha256': sha(thin)}, 'actual selected Rust wire input differs')
    require(all(file_pin(pin['path']) == {key: pin[key] for key in ('bytes', 'sha256')}
                for pin in receipt['tools'].values()), 'actual Cargo/Rust input changed before link')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['build', 'verify'])
    {'build': build, 'verify': verify}[parser.parse_args().action]()
