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
from fork_input import decode, require, sha
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


# Exact selectors come from the pinned sagernet/gomobile v0.1.13 generator.
# This data authorizes a source binding variant, never a device operation.
NATIVE_TUN_CONTRACT = 'polaris-native-tun-v1'
NATIVE_TUN_CONDITION = 'POLARIS_NATIVE_TUN_V1'
NATIVE_TUN_ARGS = 'PolarisNativeTunCompiler.args'


def native_tun_abi_present(headers):
    texts = [re.sub(r'/\*.*?\*/|//[^\n]*', ' ', text, flags=re.DOTALL)
             for text in headers]
    names = r'\b(?:LibboxRuntimeTun(?:Platform|Session|Receipt)|setRuntimeTunPlatform|newTunSession)\b'
    if not any(re.search(names, text) for text in texts):
        return False
    for text in texts:
        if not re.search(names, text):
            continue
        # Generated include guards are the only supported conditional here.
        # A disabled declaration or handwritten feature macro cannot attest ABI.
        conditions = re.findall(r'(?m)^\s*#\s*(if|ifdef|ifndef|elif|else|endif)\b([^\n]*)', text)
        guards = [tail.strip() for directive, tail in conditions if directive == 'ifndef']
        require(not conditions or
                (len(conditions) == 2 and len(guards) == 1 and
                 re.fullmatch(r'__[A-Za-z0-9_]+_H__', guards[0]) and
                 conditions[0][0] == 'ifndef' and conditions[1][0] == 'endif' and
                 not conditions[1][1].strip() and
                 re.search(r'(?m)^\s*#\s*define\s+' + re.escape(guards[0]) + r'\s*$', text)),
                'native TUN declarations use unsupported preprocessor conditions')
    text = '\n'.join(re.sub(r'(?m)^[ \t]*#[^\n]*', '', part.replace('\\\n', ''))
                     for part in texts)

    def block(kind, name, protocols):
        blocks = []
        for match in re.finditer(r'@' + kind + r'\s+' + name + r'\b([^\n]*)', text):
            tail = match.group(1).strip()
            if tail.startswith(';'):
                continue  # A forward declaration supplies no methods.
            end = re.search(r'@end\b', text[match.end():])
            require(end is not None, 'native TUN declaration is unterminated: ' + name)
            body = text[match.end():match.end() + end.start()]
            require(not re.search(r'@(?:protocol|interface)\b', body),
                    'native TUN declaration overlaps another block: ' + name)
            if kind == 'protocol':
                require(re.fullmatch(r'<\s*NSObject\s*>', tail),
                        'native TUN protocol inheritance differs: ' + name)
            else:
                inheritance = re.fullmatch(r':\s*NSObject\s*<([^>]+)>\s*(?:\{)?', tail)
                require(inheritance is not None, 'native TUN class inheritance differs: ' + name)
                actual = [part.strip() for part in inheritance.group(1).split(',')]
                require(len(actual) == len(set(actual)) and all(p in actual for p in protocols),
                        'native TUN class protocol identity differs: ' + name)
            blocks.append(body)
        require(len(blocks) == 1, 'native TUN declaration missing or duplicated: ' + name)
        return blocks[0]

    def tokens(value):
        return re.findall(r'@[A-Za-z_]\w*|[A-Za-z_]\w*|[^\s]', value)

    def methods(body, expected):
        declarations = re.findall(r'[-+]\s*\([^;]*?;', body)
        for name, declaration in expected.items():
            candidates = [part for part in declarations
                          if re.match(r'[-+]\s*\([^)]*\)\s*' + name + r'\b', part)]
            require(len(candidates) == 1 and tokens(candidates[0]) == tokens(declaration),
                    'native TUN selector/return ABI differs: ' + name)

    platform = {
        'runtimeTunVersion': '- (int32_t)runtimeTunVersion;',
        'newTunSession': '- (id<LibboxRuntimeTunSession> _Nullable)newTunSession:'
            '(id<LibboxTunOptions> _Nullable)options previous:'
            '(id<LibboxRuntimeTunSession> _Nullable)previous receipt:'
            '(LibboxRuntimeTunReceipt* _Nullable)receipt error:'
            '(NSError* _Nullable* _Nullable)error;',
    }
    session = {'ownerID': '- (NSString* _Nonnull)ownerID;',
               'fileDescriptor': '- (int32_t)fileDescriptor;'}
    session.update({name: '- (BOOL)' + name + ':(NSError* _Nullable* _Nullable)error;'
                    for name in ('prepare', 'validate', 'abort', 'retire')})
    for name, expected in (('RuntimeTunPlatform', platform), ('RuntimeTunSession', session)):
        methods(block('protocol', 'Libbox' + name, ()), expected)
        methods(block('interface', 'Libbox' + name, ('goSeqRefInterface', 'Libbox' + name)), expected)
    methods(block('interface', 'LibboxRuntimeTunReceipt', ('goSeqRefInterface',)),
            {'ownerID': '- (NSString* _Nonnull)ownerID;', 'published': '- (BOOL)published;'})
    methods(block('interface', 'LibboxCommandServer', ('goSeqRefInterface',)),
            {'setRuntimeTunPlatform': '- (BOOL)setRuntimeTunPlatform:'
                '(id<LibboxRuntimeTunPlatform> _Nullable)platform error:'
                '(NSError* _Nullable* _Nullable)error;'})
    return True


def compiler_args_path():
    return paths()[0].with_name(NATIVE_TUN_ARGS)


def compiler_args(present):
    return ('-D\n' + NATIVE_TUN_CONDITION + '\n').encode() if present else b''


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


def selected_inputs():
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
    return binary, selected, header_inputs


def prepare():
    binary, selected, header_inputs = selected_inputs()
    symbols = run(['xcrun', 'nm', '--no-llvm-bc', '-gU', binary])
    headers = [raw.decode() for raw, _ in header_inputs.values()]
    present = capability_present(headers, symbols)
    native_tun = native_tun_abi_present(headers)
    flags, receipt = paths()
    swift_args = compiler_args_path()
    swift_args.write_bytes(compiler_args(native_tun))
    # Explicitly root this one optional function; do not change the global export policy.
    flags.write_text('-Xlinker\n-u\n-Xlinker\n' + SYMBOL + '\n' if present else '')
    receipt.write_text(json.dumps({'capabilityPresent': present, 'binary': selected['binary'],
                                  'headers': {name: pin for name, (_, pin) in header_inputs.items()},
                                  'flags': file_pin(flags, allow_empty=True), 'devicePassed': False,
                                  'nativeTun': {'contractVersion': NATIVE_TUN_CONTRACT,
                                                'present': native_tun,
                                                'compilerArgs': file_pin(swift_args, allow_empty=True)}}) + '\n')


def verify(receipt=None):
    flags, receipt_path = paths()
    if receipt is None:
        receipt = read_receipt(receipt_path)
    require(file_pin(flags, allow_empty=True) == receipt['flags'], 'TLS capability linker flags changed')
    require(file_pin(compiler_args_path(), allow_empty=True) == receipt['nativeTun']['compilerArgs'],
            'native TUN compiler response file changed')
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
                      'nativeTunHeaderABIAvailable': receipt['nativeTun']['present'],
                      'loadedQueryExecuted': False, 'devicePassed': False}))


def prepared_inputs():
    """Pinned parent observation, never a lease descriptor carried through Xcode."""
    root = Path(os.environ['POLARIS_TLS_PREPARED_ROOT'])
    flags, receipt = root / 'PolarisTLSFingerprintLinker.args', root / 'PolarisTLSFingerprintLinker.json'
    require(receipt.stat().st_size <= 65536 and flags.stat().st_size <= 256,
            'prepared TLS input size budget exceeded')
    raw, pin = read_pinned(receipt)
    require(pin['sha256'] == os.environ['POLARIS_TLS_PREPARED_SHA256'],
            'parent TLS capability receipt changed')
    value = decode(raw)
    require(type(value) is dict and
            set(value) == {'capabilityPresent', 'binary', 'headers', 'flags', 'devicePassed', 'nativeTun'} and
            type(value.get('capabilityPresent')) is bool and value.get('devicePassed') is False,
            'prepared TLS capability receipt structure differs')
    flag_raw, flag_pin = read_pinned(flags, allow_empty=True)
    require(flag_pin == value['flags'], 'parent TLS capability flags changed')
    binary, selected, headers = selected_inputs()
    require(selected['binary'] == value['binary'] and
            {name: pin for name, (_, pin) in headers.items()} == value['headers'],
            'prepared TLS capability inputs differ from selected framework')
    expected = ('-Xlinker\n-u\n-Xlinker\n' + SYMBOL + '\n' if value['capabilityPresent'] else '').encode()
    require(flag_raw == expected, 'unexpected TLS capability linker flags')
    present = native_tun_abi_present([content.decode() for content, _ in headers.values()])
    swift_args = root / NATIVE_TUN_ARGS
    native = value['nativeTun']
    require(type(native) is dict and set(native) == {'contractVersion', 'present', 'compilerArgs'} and
            native['contractVersion'] == NATIVE_TUN_CONTRACT and type(native['present']) is bool and
            native['present'] == present, 'prepared native TUN ABI decision differs from actual headers')
    require(swift_args.stat().st_size <= 128, 'prepared native TUN compiler arguments exceed budget')
    swift_raw, swift_pin = read_pinned(swift_args, allow_empty=True)
    require(swift_pin == native['compilerArgs'], 'parent native TUN compiler response file changed')
    require(swift_raw == compiler_args(present), 'unexpected native TUN compiler arguments')
    return flag_raw, swift_raw, raw


def install():
    # Xcode's service can preserve an FD number in env after closing/reusing its
    # descriptor. This hook only checks/copies bytes; native tools run in the
    # admitted parent before archive and again against the final archived image.
    flag_raw, swift_raw, receipt_raw = prepared_inputs()
    flags, receipt = paths()
    swift_args = compiler_args_path()
    require(not flags.is_symlink() and not receipt.is_symlink() and not swift_args.is_symlink(),
            'TLS/native compiler output redirected')
    flags.write_bytes(flag_raw)
    swift_args.write_bytes(swift_raw)
    receipt.write_bytes(receipt_raw)


def verify_installed():
    flag_raw, swift_raw, receipt_raw = prepared_inputs()
    flags, receipt = paths()
    require(read_pinned(flags, allow_empty=True)[0] == flag_raw and
            read_pinned(receipt)[0] == receipt_raw, 'installed TLS linker inputs changed')
    require(read_pinned(compiler_args_path(), allow_empty=True)[0] == swift_raw,
            'installed native TUN compiler input changed')
    print(json.dumps({'scope': 'installed TLS linker input bytes only; final native verification belongs to parent',
                      'loadedQueryExecuted': False, 'devicePassed': False}))


def native_action(action):
    from budget import reservation
    from producer import ACTIVE_FDS, private_dir
    from fork_input import load
    require(os.environ.get('POLARIS_BUILD_RESERVATION_FD') is not None and
            os.environ.get('POLARIS_FORMAL_APP_BUILD') == '1',
            'native TLS inspection must inherit admitted parent reservation')
    with reservation(private_dir(Path(os.environ['POLARIS_APP_WORK'])), load(), ACTIVE_FDS):
        if action == 'verify':
            # Bind final tools to the immutable pre-archive capability decision.
            _, _, receipt_raw = prepared_inputs()
            require(Path(os.environ['DERIVED_FILE_DIR']).resolve(strict=True) ==
                    Path(os.environ['POLARIS_TLS_PREPARED_ROOT']).resolve(strict=True),
                    'final TLS verifier must use parent prepared inputs')
            verify(decode(receipt_raw))
        else:
            prepare()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'verify', 'install', 'verify-installed'])
    action = parser.parse_args().action
    if action in ('prepare', 'verify'):
        native_action(action)
    else:
        {'install': install, 'verify-installed': verify_installed}[action]()
