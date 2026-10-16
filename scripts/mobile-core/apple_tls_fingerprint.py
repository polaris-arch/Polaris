#!/usr/bin/env python3
"""Retain the optional loaded-libbox query in the final PacketTunnel executable."""
import argparse
import json
import os
from pathlib import Path
import re
import sys

sys.dont_write_bytecode = True
from artifacts import file_pin, read_pinned, read_receipt
from fork_input import require, sha
from producer import run

SYMBOL = '_LibboxTLSFingerprintCapabilities'
FUNCTION = SYMBOL[1:]


def capability_present(headers, symbols):
    # Inspect actual declarations only: comments cannot attest an ABI.
    texts = [re.sub(r'/\*.*?\*/|//[^\n]*', ' ', text, flags=re.DOTALL) for text in headers]
    texts = [re.sub(r'(?m)^[ \t]*#[^\n]*', '', text.replace('\\\n', '')) for text in texts]
    texts = [re.sub(r'\bNS_ASSUME_NONNULL_(?:BEGIN|END)\b', '', text) for text in texts]
    declarations = [part.strip() for text in texts for part in re.split(r'[;{}]', text)
                    if re.search(r'\b' + FUNCTION + r'\s*\(', part)]
    defined = bool(re.search(r'\bT\s+' + SYMBOL + r'\s*$', symbols, re.MULTILINE))
    require(bool(declarations) == defined, 'libbox TLS capability declaration/definition mismatch')
    if declarations:
        require(len(declarations) == 1, 'libbox TLS capability declaration is not unique')
        require(re.fullmatch(r'(?:FOUNDATION_EXPORT\s+)?NSString\s*\*\s*_Nonnull\s+' + FUNCTION +
                             r'\s*\(\s*NSError\s*\*\s*_Nullable\s*\*\s*_Nullable\s+\w+\s*\)',
                             declarations[0]),
                'libbox TLS capability C ABI differs from the optional Swift bridge')
    return defined


def paths():
    folder = Path(os.environ['DERIVED_FILE_DIR'])
    folder.mkdir(parents=True, exist_ok=True)
    return folder / 'PolarisTLSFingerprintLinker.args', folder / 'PolarisTLSFingerprintLinker.json'


def framework_path(framework, path):
    # The preceding verify-link phase admits this selected framework. Follow
    # its versioned aliases only within that boundary; file_pin stays strict.
    require(framework.is_dir() and not framework.is_symlink(), 'libbox framework root redirected/missing')
    try:
        boundary = framework.resolve(strict=True)
        resolved = path.resolve(strict=True)
    except (OSError, RuntimeError):
        raise RuntimeError('libbox framework input missing or unresolvable') from None
    require(resolved.is_relative_to(boundary), 'libbox input escaped selected framework')
    return resolved


def prepare():
    context = Path(os.environ['POLARIS_MOBILE_LINK_INPUT'])
    require(sha(context.read_bytes()) == os.environ['POLARIS_MOBILE_LINK_INPUT_SHA256'],
            'actual mobile link context changed before TLS capability preparation')
    link = read_receipt(context)
    platform = os.environ['PLATFORM_NAME']
    require(platform in ('iphoneos', 'iphonesimulator') and os.environ['ARCHS'] == 'arm64',
            'unrequested PacketTunnel TLS capability target')
    identifier = 'ios-arm64' if platform == 'iphoneos' else 'ios-arm64-simulator'
    selected = link['componentOutput']['slices'][identifier]
    require(selected['libraryIdentifier'] == identifier, 'selected libbox slice differs from PacketTunnel target')
    framework = Path(link['framework']) / identifier / 'Libbox.framework'
    binary = framework_path(framework, framework / 'Libbox')
    require(file_pin(binary) == selected['binary'], 'actual libbox link bytes differ')
    header_root = framework_path(framework, framework / 'Headers')
    headers = [framework_path(framework, path) for path in sorted(header_root.glob('*.h'))]
    require(0 < len(headers) <= 64 and sum(p.stat().st_size for p in headers) <= 2 * 1024 * 1024,
            'libbox generated header inventory exceeds budget')
    header_inputs = {p.name: read_pinned(p) for p in headers}
    symbols = run(['xcrun', 'nm', '--no-llvm-bc', '-gU', binary])
    present = capability_present([raw.decode() for raw, _ in header_inputs.values()], symbols)
    flags, receipt = paths()
    # Explicitly root this one optional function; do not change the global export policy.
    flags.write_text('-Xlinker\n-u\n-Xlinker\n' + SYMBOL + '\n' if present else '')
    receipt.write_text(json.dumps({'capabilityPresent': present, 'binary': selected['binary'],
                                  'headers': {name: pin for name, (_, pin) in header_inputs.items()},
                                  'flags': file_pin(flags, allow_empty=True), 'devicePassed': False}) + '\n')


def verify():
    flags, receipt_path = paths()
    receipt = read_receipt(receipt_path)
    require(file_pin(flags, allow_empty=True) == receipt['flags'], 'TLS capability linker flags changed')
    executable = Path(os.environ['TARGET_BUILD_DIR']) / os.environ['EXECUTABLE_PATH']
    if receipt['capabilityPresent']:
        symbols = run(['xcrun', 'nm', '--no-llvm-bc', '-gU', executable])
        require(re.search(r'\bT\s+' + SYMBOL + r'\s*$', symbols, re.MULTILINE),
                'final PacketTunnel stripped the actual TLS capability function')
        exports = run(['xcrun', 'dyld_info', '-exports', executable])
        require(re.search(r'(?<![\w])' + SYMBOL + r'(?![\w])', exports),
                'final PacketTunnel TLS capability function is not dynamically discoverable')
    print(json.dumps({'scope': 'actual-final-PacketTunnel-static-link',
                      'capabilityPresent': receipt['capabilityPresent'], 'executable': file_pin(executable),
                      'loadedQueryExecuted': False, 'devicePassed': False}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'verify'])
    {'prepare': prepare, 'verify': verify}[parser.parse_args().action]()
