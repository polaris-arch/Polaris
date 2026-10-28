#!/usr/bin/env python3
"""Finite synthetic byte/contracts tests; no Apple tool or target is executed."""
import copy
import importlib.util
import json
from pathlib import Path
import struct
import sys

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location('apple_carrier', Path(__file__).with_name('apple-carrier.py'))
carrier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(carrier)


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def varint(value):
    data = bytearray()
    while value > 127:
        data.append((value & 127) | 128)
        value >>= 7
    return bytes(data) + bytes([value])


def inline(raw, version='go1.25.5'):
    framed = carrier.INFO_START + raw.encode() + carrier.INFO_END
    payload = carrier.INFO + b'\x08\x02' + bytes(16)
    payload += varint(len(version)) + version.encode() + varint(len(framed)) + framed
    return payload + bytes(-len(payload) % 16)


def mach_object(target, metadata=None, build_id=None, extra=(), platform=True, kind=1, sdk=None, load_commands=(), text_payload=None):
    """Small structural fixture in the observed encoding, never compiler evidence."""
    cpu, subcpu, platform_id, _, _, _ = carrier.TARGETS[target]
    text = carrier.ID_START + build_id.encode() + carrier.ID_END if build_id is not None else b'native-A'
    if text_payload is not None:
        text = text_payload
    payloads = [('__TEXT', '__text', text)]
    if metadata is not None:
        payloads.append(('__DATA', '__go_buildinfo', metadata))
    payloads.extend(extra)
    segment_size = 72 + 80 * len(payloads)
    commands_size = segment_size + (24 if platform else 0) + sum(map(len, load_commands))
    offset = 32 + commands_size
    section_rows, body = [], bytearray()
    for segment, section, payload in payloads:
        section_rows.append(struct.pack('<16s16sQQ8I', section.encode(), segment.encode(),
            offset, len(payload), offset, 0, 0, 0, 0, 0, 0, 0))
        body += payload
        offset += len(payload)
    segment = struct.pack('<II16s4Q4I', 0x19, segment_size, b'', 0, offset,
        32 + commands_size, len(body), 7, 7, len(payloads), 0) + b''.join(section_rows)
    command = struct.pack('<6I', 0x32, 24, platform_id, 15 << 16, (27 << 16) | (1 << 8) if sdk is None else sdk, 0) if platform else b''
    return struct.pack('<8I', 0xfeedfacf, cpu, subcpu, kind, 1 + int(platform) + len(load_commands), commands_size, 0, 0) + segment + command + b''.join(load_commands) + body


def archive(members):
    result = carrier.AR
    for name, data in members:
        name_length = (len(name) + 1 + 3) // 4 * 4
        named = name.encode() + bytes(name_length - len(name))
        payload = named + data
        header = (f'#1/{name_length:<13}'[:16] + f'{0:<12}{0:<6}{0:<6}{0o644:<8o}{len(payload):<10}' + '`\n').encode()
        check(len(header) == 60, 'fixture ar header size')
        result += header + payload + (b'\n' if len(payload) % 2 else b'')
    return result


def fat(first, second):
    offset1 = 48
    offset2 = (offset1 + len(first) + 7) // 8 * 8
    header = carrier.FAT + struct.pack('>I', 2)
    header += struct.pack('>5I', 0x1000007, 3, offset1, len(first), 3)
    header += struct.pack('>5I', 0x100000c, 0, offset2, len(second), 3)
    return header + first + bytes(offset2 - offset1 - len(first)) + second


def fixture():
    helper = carrier.source_helpers()
    root = Path(__file__).parent.parent
    source = json.loads((root / 'scripts/libbox-patches/source-manifest.json').read_bytes())
    core = json.loads((root / 'src-tauri/core-manifest.json').read_bytes())
    receipt = {'schema': 'polaris-core-source-v1', 'sourceGraphState': 'dependencies-patched',
        'graphScope': 'declared-patched-modules', 'moduleGraphQueries': sorted(core['sourceBuild']['dependencyModules']),
        'sourceCommit': source['sourceCommit'], 'sourceURL': 'https://github.com/SagerNet/sing-box',
        'upstreamTree': '1' * 40, 'patches': source['patches'],
        'dependencies': [{**d, 'upstreamTree': '1' * 40, 'replacement': './polaris-dependencies/' + d['name']}
                         for d in source['dependencyPatches']],
        'moduleGraph': [{'Path': d['module'], 'Version': d['upstreamVersion'],
                         'Replace': {'Path': './polaris-dependencies/' + d['name']}}
                        for d in source['dependencyPatches']]}
    for key in ('sourceManifestSha256', 'provisionerSha256', 'patchedSourceTree', 'buildTree', 'mainGoModSha256', 'mainGoSumSha256'):
        receipt[key] = core['sourceBuild'][key]
    receipt['moduleGraphSha256'] = helper.digest(helper.canonical(receipt['moduleGraph']))
    receipt['fingerprint'] = helper.digest(helper.canonical(receipt))
    core['sourceBuild'].update(sourceReceiptFingerprint=receipt['fingerprint'], moduleGraphSha256=receipt['moduleGraphSha256'])
    expected_id = helper.provider().source_linker_flag(receipt).removeprefix('-buildid=')
    policy = {'schema': 'polaris-apple-carrier-contract-v1', 'evidenceScope': 'carrier-inspection-only',
              'sourceReceiptFingerprint': receipt['fingerprint'], 'targets': {}}
    raw, objects, archives = {}, {}, {}
    for target, (_, _, _, arch, sdk, variant) in carrier.TARGETS.items():
        # Full 2+6 synthetic graph exercises the validator, not Apple's real graph.
        rows = ['path\texample.invalid/synthetic-generated-main',
                'mod\texample.invalid/synthetic-generated-main\t(devel)\t']
        for dep in source['dependencyPatches']:
            rows += ['dep\t' + dep['module'] + '\t' + dep['upstreamVersion'] + '\t',
                     '=>\t./polaris-dependencies/' + dep['name'] + '\t(devel)\t']
        rows += ['dep\t' + module + '\t' + version + '\th1:synthetic-checksum'
                 for module, version in core['sourceBuild']['transportPins'].items()]
        tags = ['ios', 'synthetic-test'] + (['iossimulator'] if variant else [])
        settings = {'-buildmode': 'c-archive', '-compiler': 'gc', '-tags': ','.join(tags),
                    '-trimpath': 'true', 'CGO_ENABLED': '1', 'GOARCH': arch, 'GOOS': 'ios',
                    'GOARM64' if arch == 'arm64' else 'GOAMD64': 'v8.0' if arch == 'arm64' else 'v1'}
        rows += ['build\t' + k + '=' + v for k, v in settings.items()]
        raw[target] = '\n'.join(rows) + '\n'
        parsed = helper.parse_build_info('fixture-section: go' + source['goVersion'] + '\n' +
                                        ''.join('\t' + row + '\n' for row in rows))
        external = {'sdk': sdk, 'sdkVersion': '27.1', 'sdkBuild': 'synthetic-sdk-build',
                    'sysrootSha256': '2' * 64, 'minOSRaw': 15 << 16, 'sdkRaw': (27 << 16) | (1 << 8),
                    'environment': {'GOOS': 'ios', 'GOARCH': arch, 'CGO_ENABLED': '1',
                                    'CGO_CFLAGS': 'synthetic ' + sdk, 'CGO_CXXFLAGS': 'synthetic ' + sdk,
                                    'CGO_LDFLAGS': 'synthetic ' + sdk}}
        policy['targets'][target] = {'carrierCount': 1, 'graphFingerprint': helper.digest(raw[target].encode()),
            'buildInfo': parsed, 'effectiveTags': tags, 'externalInputs': external,
            'patchedModules': {'requiredLinked': list(core['sourceBuild']['dependencyModules']), 'allowedAbsent': []},
            'transportModules': {'requiredLinked': list(core['sourceBuild']['transportPins']), 'confirmedAbsent': []}}
        objects[target] = mach_object(target, inline(raw[target]), expected_id)
        archives[target] = archive([('__.SYMDEF SORTED', bytes(8)), ('identity-without-go-name.o', objects[target]),
                                  ('same.o', mach_object(target)),
                                  ('same.o', mach_object(target, extra=[('__TEXT', '__cstring', b'B')]))])
    tools = {'targets': {t: p['externalInputs'] for t, p in policy['targets'].items()}}
    return {'source': source, 'core': core, 'receipt': receipt, 'policy': policy, 'tools': tools,
            'id': expected_id, 'raw': raw, 'objects': objects, 'archives': archives}


def run_tests():
    f = fixture()
    helper, count = carrier.source_helpers(), 0
    device, sim, amd = 'ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'

    def inspect(data, targets=(device,), policy=None, tools=None, receipt=None):
        return carrier.inspect_carrier(data, list(targets), receipt or f['receipt'], f['source'], f['core'],
            f['policy'] if policy is None else policy, f['tools'] if tools is None else tools)

    def positive(action):
        nonlocal count
        result = action()
        check(result['evidenceScope'] == 'carrier-inspection-only', 'fixture promoted artifact scope')
        count += 1
        return result

    def rejected(action, reason):
        nonlocal count
        try:
            action()
        except RuntimeError as error:
            check(reason in str(error), 'wrong rejection: ' + str(error) + '; expected ' + reason)
        else:
            raise AssertionError('Counterexample accepted: ' + reason)
        count += 1

    for target in carrier.TARGETS:
        for data in (f['objects'][target], f['archives'][target]):
            actual = positive(lambda data=data, target=target: inspect(data, (target,)))
            row = actual['targets'][target]
            check(row['goCarriers'][0]['buildID'] == f['id'], 'provider identity mismatch')
            check(row['goCarriers'][0]['parsedBuildInfo'] == f['policy']['targets'][target]['buildInfo'], 'metadata lost')
            if len(row['members']) > 1:
                repeated = [m for m in row['members'] if m['name'] == 'same.o']
                check(len(repeated) == 2 and repeated[0]['ordinal'] != repeated[1]['ordinal']
                      and repeated[0]['payloadOffset'] != repeated[1]['payloadOffset']
                      and repeated[0]['sha256'] != repeated[1]['sha256'], 'duplicate member lost')
    two = fat(f['archives'][amd], f['archives'][sim])
    got = positive(lambda: inspect(two, (sim, amd)))
    check(got['targets'][amd]['sliceOrdinal'] == 0 and got['targets'][sim]['sliceOrdinal'] == 1, 'input order trusted')
    # No member inventory count/name is frozen; insert another valid native member.
    data = archive([('renamed-carrier.o', f['objects'][device]), ('more-native.o', mach_object(device))])
    positive(lambda: inspect(data))
    # Match the actual C0 final Mach-O envelope without claiming a compiled sample.
    positive(lambda: inspect(mach_object(device, inline(f['raw'][device]), f['id'], kind=2)))
    missing_platform = mach_object(device, platform=False)
    p = copy.deepcopy(f['policy'])
    p['targets'][device]['externalInputs']['nativeMembersWithoutPlatform'] = [helper.digest(missing_platform)]
    bound_tools = {'targets': {t: v['externalInputs'] for t, v in p['targets'].items()}}
    positive(lambda: inspect(archive([('go-other.o', f['objects'][device]), ('native.o', missing_platform)]), policy=p, tools=bound_tools))
    # Section SDK0 occurs in observed native assembler members, not Go carriers.
    positive(lambda: inspect(archive([('carrier.o', f['objects'][device]), ('assembly.o', mach_object(device, sdk=0))])))
    base = f['objects'][device]

    def changed(data, offset, fmt, value):
        bad = bytearray(data)
        struct.pack_into(fmt, bad, offset, value)
        return bytes(bad)

    for length, reason in ((0, 'unsupported container'), (4, 'Mach-O header'), (31, 'Mach-O header'),
                           (40, 'load-command region'), (len(base) - 1, 'segment file')):
        rejected(lambda length=length: inspect(base[:length]), reason)
    for off, fmt, value, reason in ((4, '<I', 12, 'CPU/subCPU'), (8, '<I', 2, 'CPU/subCPU'),
            (12, '<I', 6, 'CPU/subCPU'), (20, '<I', 0xffffffff, 'load-command region'),
            (16, '<I', 0xffffffff, 'load-command count'), (28, '<I', 1, 'CPU/subCPU'),
            (32, '<I', 0x777, 'unsupported load command'), (36, '<I', 4, 'load-command size'),
            (36, '<I', 80, 'segment layout'), (96, '<I', 0xffffffff, 'segment layout'),
            (72, '<Q', 1 << 63, 'segment file'), (136, '<Q', 1 << 63, 'section address/alignment'),
            (152, '<I', 0, 'section outside segment/header'), (156, '<I', 32, 'section address/alignment'),
            (160, '<I', len(base), 'section relocation'), (164, '<I', 1, 'section relocation'),
            (168, '<I', 0xff, 'unsupported section type')):
        rejected(lambda off=off, fmt=fmt, value=value: inspect(changed(base, off, fmt, value)), reason)
    # Both sections have the same file offset but different virtual addresses.
    rejected(lambda: inspect(changed(base, 232, '<I', 288)), 'overlap')
    bad = bytearray(base); bad[120:136] = b'__DATA\0' + bytes(9)
    rejected(lambda: inspect(bytes(bad)), 'missing/extra Go identity')
    bad = bytearray(base); bad[200:216] = b'__TEXT\0' + bytes(9)
    rejected(lambda: inspect(bytes(bad)), 'BuildInfo locus')
    platform_offset = 32 + 72 + 160
    for off, value, reason in ((platform_offset + 4, 16, 'platform command size'),
                              (platform_offset + 8, 7, 'platform/SDK'),
                              (platform_offset + 12, 16 << 16, 'platform/SDK'),
                              (platform_offset + 16, 0, 'platform/SDK'),
                              (platform_offset + 20, 1, 'platform command layout')):
        rejected(lambda off=off, value=value: inspect(changed(base, off, '<I', value)), reason)
    # Inline framing, UTF-8, padding and 64-bit varint rejection are independent.
    payload = inline(f['raw'][device])
    info_cases = [(payload[:14] + b'\x04' + payload[15:], 'encoding'),
                  (payload[:15] + b'\x03' + payload[16:], 'encoding'),
                  (payload[:16] + b'\x01' + payload[17:], 'encoding'),
                  (b'x' + payload[1:], 'header'), (payload[:32] + b'\x80', 'inline varint'),
                  (payload[:32] + b'\x80' * 9 + b'\x02', 'overflow'),
                  (payload[:32] + b'\x80\0', 'noncanonical'),
                  (payload[:32] + b'\x7fabc', 'inline string'),
                  (payload[:-1] + b'x', 'padding'), (payload + bytes(16), 'padding'),
                  (payload.replace(carrier.INFO_START, bytes(16)), 'module framing'),
                  (payload.replace(b'go1.25.5', b'go1.2x.5'), 'raw framing'),
                  (payload.replace(b'synthetic-test', b'\xffynthetic-test'), 'UTF-8')]
    for bad, reason in info_cases:
        rejected(lambda bad=bad: inspect(mach_object(device, bad, f['id'])), reason)
    fake = mach_object(device, extra=[('__TEXT', '__cstring', carrier.ID_START + f['id'].encode() + carrier.ID_END)])
    rejected(lambda: inspect(fake), 'missing/extra Go carrier')
    for data, reason in ((mach_object(device, inline(f['raw'][device]), None), 'missing/extra Go identity'),
                         (mach_object(device, None, f['id']), 'missing/extra Go identity'),
                         (mach_object(device, inline(f['raw'][device]), f['id'][:-1] + '0'), 'source BuildID'),
                         (mach_object(device, inline(f['raw'][device]), 'polaris-format-probe-v1-fixture'), 'source BuildID'),
                         (mach_object(device, inline(f['raw'][device]), f['id'] + '"'), 'Go BuildID value'),
                         (mach_object(device, inline(f['raw'][device]), f['id'], platform=False), 'platform missing'),
                         (archive([('one.o', base), ('two.o', base)]), 'missing/extra Go carrier'),
                         (archive([('one.o', base), ('two.o', f['objects'][amd])]), 'mixed member CPU')):
        rejected(lambda data=data: inspect(data), reason)
    rejected(lambda: inspect(mach_object(device, inline(f['raw'][device]), f['id'],
               extra=[('__TEXT', '__text', carrier.ID_START + f['id'].encode() + carrier.ID_END)])), 'duplicate section')
    ar = f['archives'][device]
    for bad, reason in ((ar[:-1], 'ar padding'), (ar[:67], 'ar header'),
                       (ar[:85] + b'x' + ar[86:], 'name/padding'),
                       (changed(ar, 56, '<B', ord('-')), 'ar numeric'),
                       (ar[:8] + b'go.o/           ' + ar[24:], 'BSD ar name'),
                       (ar[:8] + b'#1/999          ' + ar[24:], 'name length'),
                       (ar[:66] + b'x\n' + ar[68:], 'header trailer')):
        rejected(lambda bad=bad: inspect(bad), reason)
    for name in ('../escape.o', '/absolute.o', '..', 'a\\b.o'):
        rejected(lambda name=name: inspect(archive([(name, base)])), 'BSD ar name/padding')
    odd = archive([('odd.o', b'x')])
    rejected(lambda: inspect(odd[:-1]), 'unsupported code member format')
    # A valid odd-length Mach-O payload exercises archive newline padding.
    odd_native = mach_object(device, extra=[('__TEXT', '__cstring', b'x')])
    odd = archive([('carrier.o', base), ('odd.o', odd_native)])
    positive(lambda: inspect(odd))
    rejected(lambda: inspect(odd[:-1] + b'x'), 'ar padding byte')
    second_offset = struct.unpack_from('>I', two, 36)[0]
    stale = two[:second_offset] + two[second_offset:].replace(f['id'].encode(), (f['id'][:-1] + '0').encode(), 1)
    rejected(lambda: inspect(stale, (sim, amd)), 'source BuildID')
    for off, value, reason in ((4, 3, 'slice count'), (28, 0x1000007, 'header/payload CPU'),
                              (36, 48, 'overlap'), (40, 0xffffffff, 'fat slice bounds'),
                              (44, 32, 'alignment/header'), (36, 4, 'alignment/header')):
        rejected(lambda off=off, value=value: inspect(changed(two, off, '>I', value), (sim, amd)), reason)
    # Wrong CPU must compare with actual payload, even when header CPU stays unique.
    rejected(lambda: inspect(changed(two, 32, '>I', 1), (sim, amd)), 'header/payload CPU')
    rejected(lambda: inspect(two[:-1], (sim, amd)), 'fat slice bounds')
    rejected(lambda: inspect(two + b'\0', (sim, amd)), 'fat trailing')
    for targets in ((), (device, device), ('unknown',), (device, sim)):
        rejected(lambda targets=targets: inspect(base, targets), 'target' if targets != (device, sim) else 'slice inventory')
    rejected(lambda: inspect(two, (device, amd)), 'platform/SDK')
    for badraw, reason in ((f['raw'][device] + 'build\tGOOS=ios\n', 'duplicate build setting'),
                           ('path\tother\n' + f['raw'][device], 'Duplicate BuildInfo path'),
                           (f['raw'][device] + 'dep\tgolang.org/x/sys\tv0.47.0\n', 'Duplicate/malformed module'),
                           ('=>\t./unbound\n' + f['raw'][device], 'Unbound/duplicate replacement'),
                           (f['raw'][device] + 'unknown\tx\n', 'Unsupported Go BuildInfo row'),
                           (f['raw'][device].replace('CGO_ENABLED=1', 'CGO_ENABLED=0'), 'Go target BuildInfo'),
                           (f['raw'][device].replace('GOARM64=v8.0', 'GOARM64=v9.0'), 'Go target BuildInfo'),
                           (f['raw'][device].replace('ios,synthetic-test', 'ios,ios'), 'effective tags'),
                           (f['raw'][device].replace('(devel)', 'v9.0.0', 1), 'complete Go metadata')):
        rejected(lambda badraw=badraw: inspect(mach_object(device, inline(badraw), f['id'])), reason)
    for key in ('CGO_ENABLED', 'GOARCH', 'GOOS', '-buildmode', '-compiler', '-trimpath'):
        raw = f['raw'][device].replace('build\t' + key + '=', 'build\tremoved-' + key + '=')
        rejected(lambda raw=raw: inspect(mach_object(device, inline(raw), f['id'])), 'Go target BuildInfo')
    for field in ('patchedModules', 'transportModules', 'carrierCount', 'graphFingerprint', 'buildInfo', 'externalInputs'):
        p = copy.deepcopy(f['policy']); del p['targets'][device][field]
        rejected(lambda p=p: inspect(base, policy=p), 'partition' if field.endswith('Modules') else 'complete target')
    p = copy.deepcopy(f['policy']); p['targets'][device]['carrierCount'] = True
    rejected(lambda: inspect(base, policy=p), 'complete target')
    for field in ('requiredLinked', 'allowedAbsent'):
        p = copy.deepcopy(f['policy']); p['targets'][device]['patchedModules'][field].append('undeclared/module')
        rejected(lambda p=p: inspect(base, policy=p), 'partition inventory')
    p = copy.deepcopy(f['policy']); p['targets'][device]['patchedModules']['allowedAbsent'] = p['targets'][device]['patchedModules']['requiredLinked'][:1]
    rejected(lambda: inspect(base, policy=p), 'partition inventory')
    for field in ('schema', 'evidenceScope', 'sourceReceiptFingerprint', 'targets'):
        p = copy.deepcopy(f['policy']); p[field] = None
        rejected(lambda p=p: inspect(base, policy=p), 'complete observed')
    p = copy.deepcopy(f['policy']); p['targets'][device]['externalInputs']['sysrootSha256'] = 'not-observed'
    rejected(lambda: inspect(base, policy=p), 'actual SDK/CGO')
    rejected(lambda: inspect(base, tools={}), 'actual SDK/CGO')
    rejected(lambda: inspect(base, policy={}, tools={}), 'complete observed')
    receipt = copy.deepcopy(f['receipt']); receipt['fingerprint'] = '0' * 64
    rejected(lambda: inspect(base, receipt=receipt), 'receipt fingerprint')
    # Rule checks cannot be bypassed by updating synthetic expected facts too.
    for raw, reason in ((f['raw'][device].replace('./polaris-dependencies/sing-tun', './foreign'), 'patched module provenance'),
                        (f['raw'][device] + 'dep\tforeign/module\tv1.0.0\n=>\t./foreign\n', 'undeclared module replacement')):
        p = copy.deepcopy(f['policy'])
        p['targets'][device]['buildInfo'] = helper.parse_build_info('section: go1.25.5\n' + ''.join('\t' + r + '\n' for r in raw.splitlines()))
        rejected(lambda raw=raw, p=p: inspect(mach_object(device, inline(raw), f['id']), policy=p), reason)
    p = copy.deepcopy(f['policy']); module = next(iter(f['core']['sourceBuild']['transportPins']))
    p['targets'][device]['transportModules']['requiredLinked'].remove(module)
    p['targets'][device]['transportModules']['confirmedAbsent'].append(module)
    rejected(lambda: inspect(base, policy=p), 'transport module provenance')
    # Exercise every observed command decoder with bounds, not a whole-file
    # marker. Empty auxiliary tables here are explicitly synthetic.
    sym = struct.pack('<6I', 2, 24, 0, 0, 0, 0)
    dynamic = struct.pack('<20I', 11, 80, *([0] * 18))
    extra_commands = [sym, dynamic,
        *[struct.pack('<4I', cmd, 16, 0, 0) for cmd in (0x80000033, 0x80000034, 0x26, 0x29)],
        struct.pack('<3I', 14, 16, 12) + b'x\0\0\0',
        struct.pack('<6I', 12, 32, 24, 0, 0, 0) + b'libX\0\0\0\0',
        struct.pack('<2I', 27, 24) + bytes(16), struct.pack('<4I', 42, 16, 0, 0),
        struct.pack('<II2Q', 0x80000028, 24, 0, 0), struct.pack('<6I', 44, 24, 0, 0, 0, 0)]
    full = mach_object(device, inline(f['raw'][device]), f['id'], load_commands=extra_commands)
    positive(lambda: inspect(full))
    negative_commands = [(sym + b'00000000', 'symbol command size/duplicate'),
        (changed(sym, 8, '<I', 0xffffffff), 'symbol table bounds'),
        (changed(sym, 12, '<I', 0xffffffff), 'symbol table bounds'),
        (changed(sym, 16, '<I', 0xffffffff), 'symbol table bounds'),
        (changed(sym, 20, '<I', 0xffffffff), 'symbol table bounds'),
        (dynamic, 'dynamic symbols without symbol table'),
        (changed(dynamic, 32, '<I', 0xffffffff), 'dynamic symbol table bounds'),
        (changed(extra_commands[2], 8, '<I', 0xffffffff), 'linkedit data bounds'),
        (changed(extra_commands[6], 8, '<I', 16), 'string bounds'),
        (extra_commands[6][:-4] + b'xxxx', 'string bounds'),
        (changed(extra_commands[7], 8, '<I', 4), 'string bounds'),
        (changed(extra_commands[-2], 8, '<Q', 0xffffffffffffffff), 'entry point bounds'),
        (changed(extra_commands[-1], 8, '<I', 0xffffffff), 'encryption region bounds'),
        (changed(extra_commands[-1], 16, '<I', 1), 'encrypted Mach-O unsupported')]
    # Size is held in the actual load-command header, independently of the
    # fixture constructor's byte list length.
    negative_commands[0] = (changed(negative_commands[0][0], 4, '<I', 32), negative_commands[0][1])
    for command, reason in negative_commands:
        rejected(lambda command=command: inspect(mach_object(device, inline(f['raw'][device]), f['id'], load_commands=[command])), reason)
    rejected(lambda: inspect(mach_object(device, inline(f['raw'][device]), f['id'],
                          load_commands=[sym, changed(dynamic, 8, '<I', 1)])), 'dynamic symbol index')
    # BSD symbol rows bind to real header offsets, keeping repeated names legal.
    sym_payload = struct.pack('<4I', 8, 0, 96, 2) + b'x\0' + bytes(6)
    with_symbols = archive([('__.SYMDEF SORTED', sym_payload), ('carrier.o', base)])
    # The member offset is obtained from bytes, never from the duplicate name.
    actual_member_offset = 8 + 60 + 20 + len(sym_payload)
    sym_payload = changed(sym_payload, 8, '<I', actual_member_offset)
    with_symbols = archive([('__.SYMDEF SORTED', sym_payload), ('carrier.o', base)])
    positive(lambda: inspect(with_symbols))
    for bad, reason in ((bytes(7), 'symbol table bounds'),
                        (struct.pack('<I', 7) + bytes(20), 'entry size'),
                        (struct.pack('<I', 80) + bytes(4), 'entries bounds'),
                        (struct.pack('<2I', 0, 80), 'strings bounds'),
                        (changed(sym_payload, 4, '<I', 99), 'symbol/member binding'),
                        (changed(sym_payload, 8, '<I', 99), 'symbol/member binding'),
                        (sym_payload + b'x', 'symbol table padding')):
        rejected(lambda bad=bad: inspect(archive([('__.SYMDEF SORTED', bad), ('carrier.o', base)])), reason)
    # Supported section types are exactly those retained in C0 inventories.
    for s_type in (2, 6, 8, 9, 11, 22):
        native = mach_object(device)
        native = changed(native, 168, '<I', s_type)
        positive(lambda native=native: inspect(archive([('carrier.o', base), ('native.o', native)])))
    note = carrier.ID_START + f['id'].encode() + carrier.ID_END
    rejected(lambda: inspect(mach_object(device, inline(f['raw'][device]), text_payload=note + note)), 'missing/extra Go identity')
    rejected(lambda: inspect(mach_object(device, inline(f['raw'][device]), text_payload=note[:-1])), 'truncated Go BuildID note')
    rejected(lambda: inspect(mach_object(device, inline(f['raw'][device]), f['id'],
                                extra=[('__DATA', '__go_buildinfo', inline(f['raw'][device]))])), 'duplicate section')
    # Allowed-absent entries still validate when present; confirmed-absent
    # entries must be absent. These are synthetic graph partitions only.
    patched = f['source']['dependencyPatches'][0]
    p = copy.deepcopy(f['policy'])
    p['targets'][device]['patchedModules']['requiredLinked'].remove(patched['module'])
    p['targets'][device]['patchedModules']['allowedAbsent'].append(patched['module'])
    wrong = f['raw'][device].replace('dep\t' + patched['module'] + '\t' + patched['upstreamVersion'],
                                      'dep\t' + patched['module'] + '\tv99.0.0')

    def bind_raw(policy, raw):
        policy['targets'][device]['buildInfo'] = helper.parse_build_info('section: go1.25.5\n' + ''.join('\t' + r + '\n' for r in raw.splitlines()))
        policy['targets'][device]['graphFingerprint'] = helper.digest(raw.encode())
        return policy

    rejected(lambda: inspect(mach_object(device, inline(wrong), f['id']), policy=bind_raw(copy.deepcopy(p), wrong)), 'patched module provenance')
    omitted = ''.join(line + '\n' for line in f['raw'][device].splitlines()
                      if not line.startswith('dep\t' + patched['module'] + '\t')
                      and not line.startswith('=>\t./polaris-dependencies/' + patched['name'] + '\t'))
    positive(lambda: inspect(mach_object(device, inline(omitted), f['id']), policy=bind_raw(p, omitted)))
    module, version = next(iter(f['core']['sourceBuild']['transportPins'].items()))
    p = copy.deepcopy(f['policy'])
    p['targets'][device]['transportModules']['requiredLinked'].remove(module)
    p['targets'][device]['transportModules']['confirmedAbsent'].append(module)
    omitted = ''.join(line + '\n' for line in f['raw'][device].splitlines() if not line.startswith('dep\t' + module + '\t'))
    positive(lambda: inspect(mach_object(device, inline(omitted), f['id']), policy=bind_raw(p, omitted)))
    wrong = f['raw'][device].replace('dep\t' + module + '\t' + version, 'dep\t' + module + '\tv99.0.0')
    rejected(lambda: inspect(mach_object(device, inline(wrong), f['id']), policy=bind_raw(copy.deepcopy(f['policy']), wrong)), 'transport module provenance')
    native = mach_object(device, extra=[('__DATA', '__bss', b'')])
    end = len(native)
    native = changed(changed(changed(changed(native, 64, '<Q', end + 16), 216, '<Q', end), 224, '<Q', 16), 232, '<I', 0)
    native = changed(native, 248, '<I', 1)
    positive(lambda: inspect(archive([('carrier.o', base), ('zero-fill.o', native)])))
    rejected(lambda: inspect(archive([('carrier.o', base), ('zero-fill.o', changed(native, 232, '<I', 1))])), 'zero-fill file offset')
    bad_tools = copy.deepcopy(f['tools'])
    bad_tools['targets'][device]['environment']['CGO_CFLAGS'] = 'simulator instead of device'
    rejected(lambda: inspect(base, tools=bad_tools), 'actual SDK/CGO input differs')
    p = copy.deepcopy(f['policy'])
    p['targets'][device]['externalInputs']['nativeMembersWithoutPlatform'] = helper.digest(base)
    bound = {'targets': {t: row['externalInputs'] for t, row in p['targets'].items()}}
    rejected(lambda: inspect(base, policy=p, tools=bound), 'native platform input inventory')
    for target, row in got['targets'].items():
        locus = row['goCarriers'][0]['loci']
        member = row['goCarriers'][0]['memberIdentity']
        check(locus['buildInfo']['containerOffset'] == row['sliceOffset'] + member['payloadOffset'] + locus['buildInfo']['offset'], 'absolute BuildInfo locus')
        check(two[locus['buildID']['containerOffset']:].startswith(carrier.ID_START), 'absolute compiler ID locus')
    print(f'{count} Apple carrier unit cases passed; no Framework or App built.')
    return count


if __name__ == '__main__':
    run_tests()
