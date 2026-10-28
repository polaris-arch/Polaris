#!/usr/bin/env python3
"""Inspect only C0-observed Apple carriers; this is not Framework/App admission."""
import importlib.util
from pathlib import Path
import re
import struct
import sys

sys.dont_write_bytecode = True
HELPER = Path(__file__).parent / 'libbox-patches/android-source.py'
MACHO = b'\xcf\xfa\xed\xfe'
AR = b'!<arch>\n'
FAT = b'\xca\xfe\xba\xbe'
INFO = b'\xff Go buildinf:'
INFO_START = bytes.fromhex('3077af0c9274080241e1c107e6d618e6')
INFO_END = bytes.fromhex('f932433186182072008242104116d8f2')
ID_START, ID_END = b'\xff Go build ID: "', b'"\n \xff'
TARGETS = {'ios-arm64': (0x100000c, 0, 2, 'arm64', 'iphoneos', None),
           'ios-arm64-simulator': (0x100000c, 0, 7, 'arm64', 'iphonesimulator', 'simulator'),
           'ios-x86_64-simulator': (0x1000007, 3, 7, 'amd64', 'iphonesimulator', 'simulator')}


def require(condition, message):
    if not condition:
        raise RuntimeError('Apple carrier: ' + message)


def source_helpers():
    spec = importlib.util.spec_from_file_location('apple_carrier_source', HELPER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def bounds(offset, size, total, label):
    require(0 <= offset <= total and 0 <= size <= total - offset, label + ' bounds')


def disjoint(ranges, label):
    end = 0
    for start, stop in sorted((a, b) for a, b in ranges if a != b):
        require(start >= end, label + ' overlap')
        end = stop


def fixed_name(raw):
    name, separator, tail = raw.partition(b'\0')
    require(not separator or not tail.strip(b'\0'), 'name padding')
    require(name and all(32 <= byte < 127 for byte in name), 'invalid section/segment name')
    return name.decode('ascii')


def inline_string(data, offset):
    value = 0
    for index in range(10):
        bounds(offset + index, 1, len(data), 'inline varint')
        byte = data[offset + index]
        require(index < 9 or byte <= 1, 'inline varint overflow')
        value |= (byte & 127) << (7 * index)
        if byte < 128:
            require(index == 0 or byte != 0, 'noncanonical inline varint')
            start = offset + index + 1
            bounds(start, value, len(data), 'inline string')
            return data[start:start + value], start + value
    raise RuntimeError('Apple carrier: inline varint overflow')


def build_info(payload, helper):
    require(len(payload) >= 32 and payload[:14] == INFO, 'BuildInfo header')
    require(payload[14:32] == b'\x08\x02' + bytes(16), 'unsupported BuildInfo encoding')
    version, stop = inline_string(payload, 32)
    framed, stop = inline_string(payload, stop)
    require(len(payload) % 16 == 0 and len(payload) - stop < 16
            and not payload[stop:].strip(b'\0'), 'BuildInfo padding')
    require(framed.startswith(INFO_START) and framed.endswith(INFO_END)
            and len(framed) > 32, 'BuildInfo module framing')
    try:
        version = version.decode('utf-8')
        raw = framed[16:-16].decode('utf-8')
    except UnicodeDecodeError as error:
        raise RuntimeError('Apple carrier: BuildInfo UTF-8') from error
    require(re.fullmatch(r'go\d+\.\d+\.\d+', version) is not None
            and raw.endswith('\n') and '\0' not in raw and '\r' not in raw, 'BuildInfo raw framing')
    # The first line is an explicit section adapter, not fabricated Go CLI output.
    adapted = 'observed-section: ' + version + '\n' + ''.join('\t' + line + '\n' for line in raw.splitlines())
    parsed = helper.parse_build_info(adapted)
    return {'goVersionRaw': version, 'moduleRaw': raw, 'parserInput': adapted,
            'parsedBuildInfo': parsed, 'buildInfoFingerprint': helper.digest(helper.canonical(parsed)),
            'headerHex': payload[:32].hex(), 'provenance': 'validated __DATA,__go_buildinfo inline section'}


def macho(data, helper):
    bounds(0, 32, len(data), 'Mach-O header')
    require(data[:4] == MACHO, 'unsupported Mach-O format')
    _, cpu, subcpu, kind, count, command_bytes, flags, reserved = struct.unpack_from('<8I', data)
    require((cpu, subcpu) in {(v[0], v[1]) for v in TARGETS.values()}
            and kind in (1, 2) and reserved == 0, 'unsupported Mach-O CPU/subCPU/filetype')
    bounds(32, command_bytes, len(data), 'load-command region')
    require(0 < count <= command_bytes // 8, 'load-command count')
    limit, position = 32 + command_bytes, 32
    sections, segments, vm_segments, platforms, commands, auxiliary = [], [], [], [], [], []
    symbols, dynamic = None, None
    for ordinal in range(count):
        bounds(position, 8, limit, 'load-command header')
        command, size = struct.unpack_from('<II', data, position)
        require(size >= 8 and size % 8 == 0, 'load-command size')
        bounds(position, size, limit, 'load-command')
        commands.append({'ordinal': ordinal, 'command': command, 'offset': position, 'bytes': size})
        if command == 0x19:
            require(size >= 72, 'segment command size')
            segment_name = data[position + 8:position + 24]
            # MH_OBJECT has one anonymous segment containing named sections.
            segment_name = fixed_name(segment_name) if segment_name.strip(b'\0') else ''
            address, vm_size, offset, length, maximum, initial, nsections, segment_flags = struct.unpack_from('<4Q4I', data, position + 24)
            require(size == 72 + nsections * 80 and vm_size >= length
                    and maximum & ~7 == 0 and initial & ~maximum == 0, 'segment layout')
            bounds(offset, length, len(data), 'segment file')
            require(address + vm_size <= 1 << 64, 'segment address overflow')
            segments.append((offset, offset + length))
            vm_segments.append((address, address + vm_size))
            for index in range(nsections):
                start = position + 72 + index * 80
                section_raw, segment_raw, addr, length_s, off, align, reloc, nreloc, sflags, r1, r2, r3 = struct.unpack_from('<16s16sQQ8I', data, start)
                section, segment = fixed_name(section_raw), fixed_name(segment_raw)
                require(not segment_name or segment_name == segment, 'section segment differs')
                require(addr >= address and length_s <= address + vm_size - addr
                        and align <= 31 and addr % (1 << align) == 0, 'section address/alignment')
                zero = sflags & 255 == 1
                require(sflags & 255 in (0, 1, 2, 6, 8, 9, 11, 22), 'unsupported section type')
                row = {'segment': segment, 'section': section, 'address': addr, 'offset': off,
                       'bytes': length_s, 'flags': sflags, 'zeroFill': zero,
                       'relocationOffset': reloc, 'relocationCount': nreloc}
                bounds(reloc, nreloc * 8, len(data), 'section relocation')
                require(nreloc or reloc == 0, 'section relocation offset without entries')
                if nreloc:
                    require(reloc >= limit, 'section relocation in headers')
                    auxiliary.append((reloc, reloc + nreloc * 8))
                if zero:
                    require(off == 0, 'zero-fill file offset')
                else:
                    bounds(off, length_s, len(data), 'section file')
                    require(off >= limit and off >= offset and length_s <= offset + length - off,
                            'section outside segment/header')
                    payload = data[off:off + length_s]
                    row['sha256'] = helper.digest(payload)
                sections.append(row)
        elif command == 0x32:
            require(size >= 24, 'platform command size')
            platform, minimum, sdk, ntools = struct.unpack_from('<4I', data, position + 8)
            require(size == 24 + ntools * 8 and platform in (2, 7), 'platform command layout')
            platforms.append({'platform': platform, 'minOSRaw': minimum, 'sdkRaw': sdk})
        elif command == 2:
            require(size == 24 and symbols is None, 'symbol command size/duplicate')
            symoff, nsyms, stroff, strsize = struct.unpack_from('<4I', data, position + 8)
            symbols = nsyms
            for off, length in ((symoff, nsyms * 16), (stroff, strsize)):
                bounds(off, length, len(data), 'symbol table')
                if length:
                    require(off >= limit, 'symbol table in headers')
                    auxiliary.append((off, off + length))
        elif command == 11:
            require(size == 80 and dynamic is None, 'dynamic symbol command size/duplicate')
            values = struct.unpack_from('<18I', data, position + 8)
            dynamic = values
            for index, width in ((6, 8), (8, 56), (10, 4), (12, 4), (14, 8), (16, 8)):
                off, entries = values[index:index + 2]
                bounds(off, entries * width, len(data), 'dynamic symbol table')
                if entries:
                    require(off >= limit, 'dynamic symbol table in headers')
                    auxiliary.append((off, off + entries * width))
        elif command in (0x80000033, 0x80000034, 0x26, 0x29):
            require(size == 16, 'linkedit command size')
            off, length = struct.unpack_from('<2I', data, position + 8)
            bounds(off, length, len(data), 'linkedit data')
            if length:
                require(off >= limit, 'linkedit data in headers')
                auxiliary.append((off, off + length))
        elif command in (12, 14):
            base = 24 if command == 12 else 12
            require(size >= base, 'dylib/dylinker command size')
            off = struct.unpack_from('<I', data, position + 8)[0]
            require(base <= off < size and b'\0' in data[position + off:position + size], 'load-command string bounds')
        elif command in (27, 42, 0x80000028, 44):
            require(size == {27: 24, 42: 16, 0x80000028: 24, 44: 24}[command], 'fixed load-command size')
            if command == 0x80000028:
                entry = struct.unpack_from('<Q', data, position + 8)[0]
                require(entry < len(data), 'entry point bounds')
            if command == 44:
                off, length, cryptid, padding = struct.unpack_from('<4I', data, position + 8)
                bounds(off, length, len(data), 'encryption region')
                require(cryptid == 0 and padding == 0, 'encrypted Mach-O unsupported')
        else:
            raise RuntimeError('Apple carrier: unsupported load command ' + hex(command))
        position += size
    require(position == limit, 'load-command total differs')
    if dynamic is not None:
        require(symbols is not None, 'dynamic symbols without symbol table')
        for index in (0, 2, 4):
            bounds(dynamic[index], dynamic[index + 1], symbols, 'dynamic symbol index')
    require(len(platforms) <= 1, 'duplicate platform command')
    require(len({(s['segment'], s['section']) for s in sections}) == len(sections), 'duplicate section')
    disjoint(segments, 'segment file')
    disjoint(vm_segments, 'segment address')
    disjoint([(s['offset'], s['offset'] + s['bytes']) for s in sections if not s['zeroFill']] + auxiliary,
             'section/auxiliary file')
    disjoint([(s['address'], s['address'] + s['bytes']) for s in sections], 'section address')
    for row in sections:
        payload = data[row['offset']:row['offset'] + row['bytes']] if not row['zeroFill'] else b''
        if row['section'] == '__go_buildinfo':
            require(row['segment'] == '__DATA' and row['flags'] & 255 == 0, 'BuildInfo locus')
            row['buildInfo'] = build_info(payload, helper)
        if (row['segment'], row['section']) == ('__TEXT', '__text'):
            notes, cursor = [], 0
            while (found := payload.find(ID_START, cursor)) != -1:
                end = payload.find(ID_END, found + len(ID_START))
                require(end != -1, 'truncated Go BuildID note')
                value = payload[found + len(ID_START):end]
                require(value and all(32 < byte < 127 and byte not in (34, 92) for byte in value), 'Go BuildID value')
                stop = end + len(ID_END)
                notes.append({'value': value.decode('ascii'), 'sectionOffset': found,
                              'fileOffset': row['offset'] + found, 'bytes': stop - found,
                              'sha256': helper.digest(payload[found:stop])})
                cursor = stop
            row['buildIDs'] = notes
    info = [s for s in sections if 'buildInfo' in s]
    ids = [(s, note) for s in sections for note in s.get('buildIDs', [])]
    require(len(info) <= 1 and len(ids) <= 1 and bool(info) == bool(ids), 'missing/extra Go identity')
    return {'kind': 'Mach-O64', 'cpu': cpu, 'subCPU': subcpu, 'filetype': kind,
            'sha256': helper.digest(data), 'bytes': len(data), 'flags': flags,
            'platformCommands': platforms, 'loadCommands': commands, 'sections': sections}


def archive(data, helper):
    require(data.startswith(AR), 'unsupported archive format')
    position, members, symbol_payload = 8, [], None
    while position < len(data):
        bounds(position, 60, len(data), 'ar header')
        header = data[position:position + 60]
        require(header[58:] == b'`\n', 'ar header trailer')
        require(all(32 <= byte < 127 for byte in header[:58]), 'ar header ASCII')
        raw_name = header[:16].decode('ascii').rstrip(' ')
        require(re.fullmatch(r'#1/[1-9][0-9]*', raw_name) is not None, 'unsupported BSD ar name')
        for field, base in ((header[16:28], 10), (header[28:34], 10), (header[34:40], 10), (header[40:48], 8), (header[48:58], 10)):
            value = field.decode('ascii').strip(' ')
            require(re.fullmatch('[0-7]+' if base == 8 else '[0-9]+', value) is not None, 'ar numeric field')
        length = int(header[48:58])
        start, name_length = position + 60, int(raw_name[3:])
        bounds(start, length, len(data), 'ar member')
        require(name_length <= length, 'BSD ar name length')
        name_data = data[start:start + name_length]
        name, _, padding = name_data.partition(b'\0')
        require(not padding.strip(b'\0') and name and all(32 <= byte < 127 for byte in name)
                and b'/' not in name and b'\\' not in name and name not in (b'.', b'..'), 'BSD ar name/padding')
        name = name.decode('ascii')
        payload_start, end = start + name_length, start + length
        payload = data[payload_start:end]
        row = {'ordinal': len(members), 'headerOffset': position, 'rawName': raw_name,
               'name': name, 'nameBytes': name_length, 'payloadOffset': payload_start,
               'bytes': len(payload), 'sha256': helper.digest(payload)}
        if name == '__.SYMDEF SORTED':
            require(not members and not payload.startswith(MACHO), 'ar symbol table position/payload')
            row['classification'] = 'symbol-table'
            symbol_payload = payload
        else:
            require(payload.startswith(MACHO), 'unsupported code member format')
            row['classification'], row['machO'] = 'code', macho(payload, helper)
            require(row['machO']['filetype'] == 1, 'ar member must be MH_OBJECT')
        if length % 2:
            bounds(end, 1, len(data), 'ar padding')
            require(data[end:end + 1] == b'\n', 'ar padding byte')
        members.append(row)
        position = end + length % 2
    require(members and any(m['classification'] == 'code' for m in members), 'empty code archive')
    if symbol_payload is not None:
        bounds(0, 8, len(symbol_payload), 'BSD symbol table')
        table_bytes = struct.unpack_from('<I', symbol_payload)[0]
        require(table_bytes % 8 == 0, 'BSD symbol table entry size')
        bounds(4, table_bytes + 4, len(symbol_payload), 'BSD symbol table entries')
        string_bytes = struct.unpack_from('<I', symbol_payload, 4 + table_bytes)[0]
        string_offset = 8 + table_bytes
        bounds(string_offset, string_bytes, len(symbol_payload), 'BSD symbol strings')
        strings = symbol_payload[string_offset:string_offset + string_bytes]
        tail = symbol_payload[string_offset + string_bytes:]
        require(len(tail) < 8 and not tail.strip(b'\0'), 'BSD symbol table padding')
        code_offsets = {m['headerOffset'] for m in members if m['classification'] == 'code'}
        for offset in range(4, 4 + table_bytes, 8):
            name_offset, member_offset = struct.unpack_from('<2I', symbol_payload, offset)
            require(name_offset < len(strings) and b'\0' in strings[name_offset:]
                    and member_offset in code_offsets, 'BSD symbol/member binding')
    return {'kind': 'bsd-ar', 'sha256': helper.digest(data), 'bytes': len(data), 'members': members}


def container(data, helper):
    if data.startswith(FAT):
        bounds(0, 8, len(data), 'fat header')
        count = struct.unpack_from('>I', data, 4)[0]
        require(count == 2, 'unsupported fat slice count')
        header_end = 8 + count * 20
        bounds(8, count * 20, len(data), 'fat table')
        slices, ranges = [], []
        for ordinal in range(count):
            cpu, subcpu, off, size, align = struct.unpack_from('>5I', data, 8 + ordinal * 20)
            require(align <= 31 and off >= header_end and off % (1 << align) == 0 and size > 0, 'fat slice alignment/header')
            bounds(off, size, len(data), 'fat slice')
            ranges.append((off, off + size))
            require(data[off:off + 8] == AR, 'unsupported fat payload; expected ar')
            slices.append({'ordinal': ordinal, 'offset': off, 'bytes': size, 'cpu': cpu,
                           'subCPU': subcpu, 'alignment': align})
        disjoint(ranges, 'fat slice')
        require(len({(s['cpu'], s['subCPU']) for s in slices}) == count, 'duplicate fat CPU/subCPU')
        cursor = header_end
        for start, stop in sorted(ranges):
            require(not data[cursor:start].strip(b'\0'), 'fat padding')
            cursor = stop
        require(cursor == len(data), 'fat trailing bytes')
        for row in slices:
            row['archive'] = archive(data[row['offset']:row['offset'] + row['bytes']], helper)
        return {'kind': 'fat-ar', 'sha256': helper.digest(data), 'bytes': len(data), 'slices': slices}
    if data.startswith(AR):
        return archive(data, helper)
    if data.startswith(MACHO):
        return macho(data, helper)
    raise RuntimeError('Apple carrier: unsupported container format')


def inspect_carrier(binary, expected_targets, source_receipt, shared_source, core, build_policy, tools=None, scratch=None):
    """One producer/cache predicate. C2 must supply the complete observed contract.

    No CLI fallback or artifact admission occurs here. ``tools['targets']`` holds
    caller-captured SDK/CGO facts; ``scratch`` is reserved and never touched.
    """
    helper = source_helpers()
    helper.validate_source_receipt(source_receipt, shared_source, core)
    require(all(source_receipt.get(k) == core['sourceBuild'].get(k) for k in
                ('mainGoModSha256', 'mainGoSumSha256')), 'source main module binding differs')
    expected_id = helper.provider().source_linker_flag(source_receipt).removeprefix('-buildid=')
    require(isinstance(expected_targets, (list, tuple)) and expected_targets
            and all(isinstance(t, str) and t in TARGETS for t in expected_targets)
            and len(set(expected_targets)) == len(expected_targets), 'target inventory')
    require(isinstance(build_policy, dict) and build_policy.get('schema') == 'polaris-apple-carrier-contract-v1'
            and build_policy.get('evidenceScope') == 'carrier-inspection-only'
            and build_policy.get('sourceReceiptFingerprint') == source_receipt['fingerprint']
            and isinstance(build_policy.get('targets'), dict)
            and set(expected_targets) <= set(build_policy['targets']) <= set(TARGETS),
            'complete observed carrier contract required (C2 pending)')
    require(isinstance(tools, dict) and isinstance(tools.get('targets'), dict), 'actual SDK/CGO target inputs required')
    data = binary if isinstance(binary, bytes) else Path(binary).read_bytes()
    actual = container(data, helper)
    slices = actual['slices'] if actual['kind'] == 'fat-ar' else [{'ordinal': 0, 'offset': 0, 'bytes': len(data),
            'archive': actual}]
    require(len(slices) == len(expected_targets), 'target/slice inventory differs')
    targets = {}
    for slice_row in slices:
        body = slice_row['archive']
        members = body['members'] if body['kind'] == 'bsd-ar' else [{'ordinal': 0, 'headerOffset': None,
            'rawName': None, 'name': None, 'payloadOffset': 0, 'bytes': body['bytes'], 'sha256': body['sha256'],
            'classification': 'code', 'machO': body}]
        code = [m for m in members if m['classification'] == 'code']
        cpus = {(m['machO']['cpu'], m['machO']['subCPU']) for m in code}
        require(len(cpus) == 1, 'mixed member CPU/subCPU')
        cpu, subcpu = next(iter(cpus))
        require('cpu' not in slice_row or (cpu, subcpu) == (slice_row['cpu'], slice_row['subCPU']), 'fat header/payload CPU differs')
        matching = [t for t in expected_targets if TARGETS[t][:2] == (cpu, subcpu)]
        require(len(matching) == 1 and matching[0] not in targets, 'ambiguous/duplicate target')
        target = matching[0]
        _, _, platform, arch, sdk, variant = TARGETS[target]
        policy = build_policy['targets'][target]
        require(isinstance(policy, dict) and type(policy.get('carrierCount')) is int and policy['carrierCount'] > 0
                and helper.match('[0-9a-f]{64}', policy.get('graphFingerprint'))
                and isinstance(policy.get('buildInfo'), dict) and isinstance(policy.get('externalInputs'), dict),
                'complete target carrier/graph contract required (C2 pending)')
        external = policy['externalInputs']
        require(helper.canonical(tools['targets'].get(target)) == helper.canonical(external), 'actual SDK/CGO input differs')
        require(external.get('sdk') == sdk and helper.match('[0-9a-f]{64}', external.get('sysrootSha256'))
                and isinstance(external.get('sdkVersion'), str) and isinstance(external.get('sdkBuild'), str)
                and type(external.get('minOSRaw')) is int and type(external.get('sdkRaw')) is int
                and isinstance(external.get('environment'), dict), 'SDK/CGO input contract')
        env = external['environment']
        missing_platform = external.get('nativeMembersWithoutPlatform', [])
        require(isinstance(missing_platform, list)
                and all(helper.match('[0-9a-f]{64}', value) for value in missing_platform)
                and len(set(missing_platform)) == len(missing_platform), 'native platform input inventory')
        require(all(env.get(k) == v for k, v in {'GOOS': 'ios', 'GOARCH': arch, 'CGO_ENABLED': '1'}.items())
                and all(isinstance(env.get(k), str) and env[k] for k in ('CGO_CFLAGS', 'CGO_CXXFLAGS', 'CGO_LDFLAGS')),
                'SDK/CGO target environment')
        helper.partition(policy.get('patchedModules'), core['sourceBuild']['dependencyModules'], 'allowedAbsent')
        helper.partition(policy.get('transportModules'), list(core['sourceBuild']['transportPins']), 'confirmedAbsent')
        carriers = []
        for member in code:
            obj = member['machO']
            infos = [s for s in obj['sections'] if 'buildInfo' in s]
            for observed in obj['platformCommands']:
                require(observed['platform'] == platform and observed['minOSRaw'] == external['minOSRaw']
                        and observed['sdkRaw'] in (0, external['sdkRaw']), 'member platform/SDK differs')
            if not obj['platformCommands']:
                require(member['sha256'] in missing_platform and not infos,
                        'member platform missing without external input binding')
            if not infos:
                continue
            require(obj['platformCommands'] and obj['platformCommands'][0]['sdkRaw'] == external['sdkRaw'], 'Go carrier platform/SDK missing')
            info_section = infos[0]
            info = info_section['buildInfo']
            facts = info['parsedBuildInfo']
            settings = facts['settings']
            require(facts['goVersion'] == shared_source['goVersion'] and settings.get('GOOS') == 'ios'
                    and settings.get('GOARCH') == arch and settings.get('CGO_ENABLED') == '1'
                    and settings.get('-buildmode') == 'c-archive' and settings.get('-compiler') == 'gc'
                    and settings.get('-trimpath') == 'true'
                    and settings.get('GOARM64' if arch == 'arm64' else 'GOAMD64') == ('v8.0' if arch == 'arm64' else 'v1')
                    and not any(k.startswith('vcs') for k in settings), 'Go target BuildInfo differs')
            tags = settings.get('-tags', '').split(',')
            require(isinstance(policy.get('effectiveTags'), list) and len(tags) == len(set(tags))
                    and sorted(tags) == sorted(policy['effectiveTags']) and 'ios' in tags
                    and ('iossimulator' in tags) == (variant == 'simulator'), 'Go effective tags differ')
            require(facts['path'] and len([m for m in facts['modules'].values() if m['kind'] == 'mod']) == 1,
                    'main/path BuildInfo missing')
            require(helper.canonical(facts) == helper.canonical(policy['buildInfo']), 'complete Go metadata differs')
            for module, row in facts['modules'].items():
                require('replacement' not in row or module in core['sourceBuild']['dependencyModules'], 'undeclared module replacement')
            for dep in shared_source['dependencyPatches']:
                row = facts['modules'].get(dep['module'])
                if row is None and dep['module'] in policy['patchedModules']['allowedAbsent']:
                    continue
                require(row and row['version'] == dep['upstreamVersion']
                        and row.get('replacement') == './polaris-dependencies/' + dep['name'], 'patched module provenance differs')
            for module, version in core['sourceBuild']['transportPins'].items():
                row = facts['modules'].get(module)
                require(row is None if module in policy['transportModules']['confirmedAbsent'] else row and row['version'] == version,
                        'transport module provenance differs')
            ids = [(s, n) for s in obj['sections'] for n in s.get('buildIDs', [])]
            id_section, note = ids[0]
            require(note['value'] == expected_id, 'source BuildID differs')
            identity = {k: v for k, v in member.items() if k != 'machO'} | {'targetID': target,
                'containerSha256': actual['sha256'], 'sliceOrdinal': slice_row['ordinal'],
                'sliceOffset': slice_row['offset'], 'sliceSha256': body['sha256'], 'cpu': cpu,
                'subCPU': subcpu, 'filetype': obj['filetype']}
            carriers.append({'memberIdentity': identity, 'buildID': note['value'],
                'sourceFingerprint': source_receipt['fingerprint'], 'rawBuildInfo': info,
                'parsedBuildInfo': facts, 'buildInfoFingerprint': info['buildInfoFingerprint'],
                'loci': {'buildInfo': {k: v for k, v in info_section.items() if k != 'buildInfo'} |
                         {'containerOffset': slice_row['offset'] + member['payloadOffset'] + info_section['offset']},
                         'buildID': note | {'segment': id_section['segment'], 'section': id_section['section'],
                         'containerOffset': slice_row['offset'] + member['payloadOffset'] + note['fileOffset']}}})
        require(len(carriers) == policy['carrierCount'], 'missing/extra Go carrier')
        targets[target] = {'architecture': arch, 'variant': variant, 'format': body['kind'],
                           'sliceOrdinal': slice_row['ordinal'], 'sliceOffset': slice_row['offset'],
                           'sliceBytes': slice_row['bytes'], 'sliceSha256': body['sha256'],
                           'sliceHeader': {k: slice_row[k] for k in ('cpu', 'subCPU', 'alignment') if k in slice_row},
                           'members': members, 'goCarriers': carriers,
                           'externalInputs': external, 'graphFingerprint': policy['graphFingerprint']}
    require(set(targets) == set(expected_targets), 'target inventory differs')
    return {'evidenceScope': 'carrier-inspection-only', 'container': {k: v for k, v in actual.items()
            if k in ('kind', 'sha256', 'bytes')}, 'targets': targets}
