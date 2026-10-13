"""One content predicate for fresh mobile outputs and their cached consumers."""
import importlib.util
import json
from pathlib import Path, PurePosixPath
import plistlib
import re
import struct
import zipfile

from fork_input import ROOT, canonical, decode, require, sha, validate

ABI = {'arm64-v8a': (2, 183, 'arm64'), 'armeabi-v7a': (1, 40, 'arm'),
       'x86': (1, 3, '386'), 'x86_64': (2, 62, 'amd64')}
APPLE = {'ios-arm64': (0x100000c, 2, 'arm64', None),
         'ios-arm64-simulator': (0x100000c, 7, 'arm64', 'simulator'),
         'ios-x86_64-simulator': (0x1000007, 7, 'amd64', 'simulator')}
MAX_ARTIFACT = 1024 ** 3


def read_receipt(path):
    path = Path(path)
    require(path.is_file() and not path.is_symlink() and 0 < path.stat().st_size <= 16 * 1024 ** 2,
            'bounded regular original receipt required')
    return decode(path.read_bytes(), maximum=16 * 1024 ** 2)


def module(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def file_pin(path, *, allow_empty=False):
    path = Path(path)
    require(path.is_file() and not path.is_symlink() and (allow_empty or path.stat().st_size > 0) and path.stat().st_size <= MAX_ARTIFACT,
            'bounded regular artifact required')
    raw = path.read_bytes()
    return {'bytes': len(raw), 'sha256': sha(raw)}


def zip_entries(archive):
    infos = archive.infolist()
    require(0 < len(infos) <= 65536 and len(infos) == len({i.filename for i in infos}), 'duplicate/empty/large ZIP inventory')
    require(sum(i.file_size for i in infos) <= 2 * MAX_ARTIFACT, 'expanded ZIP too large')
    for item in infos:
        p = PurePosixPath(item.filename)
        require(not p.is_absolute() and all(part not in ('', '.', '..') for part in p.parts) and
                '\\' not in item.filename and not item.flag_bits & 1 and
                (item.external_attr >> 16) & 0o170000 != 0o120000, 'unsafe ZIP member')
    return infos


def module_stream(raw):
    decoder, position, rows = json.JSONDecoder(), 0, []
    while position < len(raw):
        while position < len(raw) and raw[position].isspace():
            position += 1
        if position == len(raw):
            break
        value, position = decoder.raw_decode(raw, position)
        require(isinstance(value, dict) and isinstance(value.get('Path'), str), 'invalid actual module graph')
        rows.append(value)
    require(rows and len({r['Path'] for r in rows}) == len(rows), 'empty/duplicate module graph')
    return rows


def component_output_identity(output):
    """Compare content while retaining the original verifier stdout observation.

    Go1.25.5 version.scanFile prints its invocation filename in the first
    line. A consumer uses its own scratch directory for the same pinned ELF.
    Only that absolute filename is observational; the version, entire module
    body, parsed facts, native bytes, BuildID, JNI/API and all other output
    fields still participate in equality. Neither original output is mutated.
    """
    result = json.loads(json.dumps(output))
    if result.get('format') != 'AAR':
        return result
    require(isinstance(result.get('nativeLibraries'), dict), 'AAR native observation inventory missing')
    for name, row in result['nativeLibraries'].items():
        require(isinstance(row, dict), 'AAR native observation row differs')
        if 'goBuildInfo' not in row:
            continue
        require(name.endswith('/libbox.so') and isinstance(row['goBuildInfo'], str),
                'raw Go observation must describe the bound libbox member')
        version = row.get('parsedBuildInfo', {}).get('goVersion')
        require(isinstance(version, str) and re.fullmatch(r'\d+\.\d+\.\d+', version),
                'raw Go observation version missing')
        header, separator, body = row['goBuildInfo'].partition('\n')
        suffix = ': go' + version
        require(separator and body.startswith('\t') and header.endswith(suffix),
                'raw Go observation header/body differs')
        filename = header[:-len(suffix)]
        require('\0' not in filename and Path(filename).is_absolute() and
                Path(filename).name == name.replace('/', '_'), 'raw Go observation filename differs')
        row['goBuildInfo'] = 'go' + version + '\n' + body
    return result


def local_replacement_aliases(replacement, source_dir, observed, name):
    literal, directory = replacement.get('Path'), replacement.get('Dir')
    diagnostic = ('native replacement differs: ' + name + '; observed=' + repr(observed) +
                  '; declaredPath=' + repr(literal) + '; declaredDir=' + repr(directory))
    require(isinstance(literal, str) and isinstance(directory, str) and
            isinstance(source_dir, str) and literal.startswith('./') and '\\' not in literal and '\0' not in literal,
            diagnostic + '; exact source-owned relative replacement required')
    components = literal[2:].split('/')
    require(components and all(part not in ('', '.', '..') for part in components),
            diagnostic + '; relative replacement escapes or is not canonical')
    root, declared_dir = Path(source_dir), Path(directory)
    require(root.is_absolute() and declared_dir.is_absolute() and '..' not in root.parts and '..' not in declared_dir.parts,
            diagnostic + '; absolute source graph roots required')
    expected = root.joinpath(*components)
    require(declared_dir == expected, diagnostic + '; source graph Path/Dir disagree')
    try:
        physical_root, physical_dir = root.resolve(strict=True), declared_dir.resolve(strict=True)
    except OSError as error:
        raise RuntimeError(diagnostic + '; declared source directory unavailable') from error
    require(physical_dir.is_dir() and physical_dir.is_relative_to(physical_root),
            diagnostic + '; replacement escaped verified source root')
    return {literal, directory}, diagnostic


def check_buildinfo(facts, value, graph, *, os_name, arch, mode, expected_id, actual_id, simulator=False):
    require(facts['goVersion'] == value['goVersion'], 'native Go version differs')
    settings = facts['settings']
    for key, expected in {'GOOS': os_name, 'GOARCH': arch, 'CGO_ENABLED': '1', '-buildmode': mode,
                          '-trimpath': 'true'}.items():
        require(settings.get(key) == expected, 'actual native setting differs: ' + key)
    profile = value['android' if os_name == 'android' else 'apple']
    platform_tags = set() if os_name == 'android' else {'ios'}
    if simulator:
        platform_tags.add('iossimulator')
    require(set(settings.get('-tags', '').split(',')) == set(profile['tags']) | platform_tags, 'native feature tags differ')
    require(actual_id == expected_id, 'actual source/tool/target BuildID input differs')
    # Go1.25.5 intentionally omits -ldflags with trimpath. Command and BuildID
    # are bound separately; do not invent that absent BuildInfo setting.
    if '-ldflags' in settings:
        from fork_input import linker_flags
        require(settings['-ldflags'] == linker_flags(value, expected_id.removeprefix('polaris-mobile-')),
                'reported native ldflags differ')
    declared = {row['Path']: row for row in graph}
    require(value['goModule'] in facts['modules'], 'original core source module absent from native carrier')
    for name, observed in facts['modules'].items():
        # The generated gobind main package is not a source module dependency.
        if name in ('command-line-arguments', 'gobind') and observed['kind'] == 'mod':
            continue
        require(name in declared, 'native module not in actual source graph: ' + name)
        expected = declared[name]
        if expected.get('Main'):
            if observed['kind'] == 'dep':
                require(observed.get('replacement') == expected['Dir'] and
                        observed['version'] == 'v0.0.0-00010101000000-000000000000',
                        'generated binding must point at the exact verified source module')
                continue
            require(observed['version'] in ('(devel)', ''), 'main module version differs')
        else:
            require(observed['version'] == expected.get('Version'), 'native module version differs: ' + name)
        replacement = expected.get('Replace')
        if replacement:
            diagnostic = ('native replacement differs: ' + name + '; observed=' + repr(observed) +
                          '; declaredPath=' + repr(replacement.get('Path')) + '; declaredDir=' + repr(replacement.get('Dir')))
            if replacement.get('Version'):
                require(observed.get('replacement') == replacement['Path'], diagnostic)
            else:
                aliases, _ = local_replacement_aliases(replacement, declared[value['goModule']].get('Dir'),
                                                    observed.get('replacement'), name)
                require(observed.get('replacement') in aliases, diagnostic)
            # Go1.25.5 debugModFromModinfo represents a versionless local
            # replacement as (devel), while go list leaves Version absent.
            # Keep that native encoding intact; versioned remote replacements
            # still require their exact source graph version and h1.
            expected_versions = {replacement['Version']} if replacement.get('Version') else {'', '(devel)'}
            require(observed.get('replacementVersion', '') in expected_versions, 'replacement version differs; ' + diagnostic)
            require(observed.get('replacementSum', '') == replacement.get('Sum', ''), 'replacement h1 differs; ' + diagnostic)
        else:
            require('replacement' not in observed, 'undeclared native module replacement')
            if expected.get('Sum'):
                require(observed.get('sum') == expected['Sum'], 'native module h1 differs: ' + name)


def elf_loads(raw):
    require(len(raw) >= 52 and raw[:6] in (b'\x7fELF\x01\x01', b'\x7fELF\x02\x01'), 'expected little-endian ELF')
    wide = raw[4] == 2
    fmt = '<HHIQQQIHHHHHH' if wide else '<HHIIIIIHHHHHH'
    header = struct.unpack_from(fmt, raw, 16)
    offset, size, count = header[4], header[8], header[9]
    entry = '<IIQQQQQQ' if wide else '<IIIIIIII'
    require(count > 0 and size == struct.calcsize(entry) and offset + size * count <= len(raw), 'ELF program header bounds')
    result = []
    for index in range(count):
        segment = struct.unpack_from(entry, raw, offset + index * size)
        if segment[0] == 1:
            off, addr, length, align = (segment[2], segment[3], segment[5], segment[7]) if wide else (segment[1], segment[2], segment[4], segment[7])
            require(off + length <= len(raw) and align > 0, 'ELF PT_LOAD bounds')
            result.append({'offset': off, 'virtualAddress': addr, 'alignment': align, 'bytes': length})
    require(result, 'ELF has no load segments')
    return result


def android_api(raw):
    wide = raw[4] == 2
    header = struct.unpack_from('<HHIQQQIHHHHHH' if wide else '<HHIIIIIHHHHHH', raw, 16)
    offset, size, count = header[4], header[8], header[9]
    result = []
    for index in range(count):
        row = struct.unpack_from('<IIQQQQQQ' if wide else '<IIIIIIII', raw, offset + index * size)
        if row[0] != 4:
            continue
        start, length = (row[2], row[5]) if wide else (row[1], row[4])
        end = start + length
        require(end <= len(raw), 'Android note bounds')
        while start < end:
            if not raw[start:end].strip(b'\0'):
                break
            require(start + 12 <= end, 'Android note header bounds')
            names, desc, kind = struct.unpack_from('<3I', raw, start)
            owner = start + 12
            payload = owner + (names + 3) // 4 * 4
            stop = payload + (desc + 3) // 4 * 4
            require(stop <= end, 'Android note payload bounds')
            if raw[owner:owner + names].rstrip(b'\0') == b'Android' and kind == 1:
                require(desc >= 4, 'Android ident descriptor missing')
                result.append(struct.unpack_from('<I', raw, payload)[0])
            start = stop
    require(len(result) == 1, 'one actual Android ident API note required')
    return result[0]


def inspect_aar(aar, value, graph, tools, run, scratch, expected_id):
    validate(value)
    helper = module('formal_mobile_buildinfo', 'scripts/libbox-patches/android-source.py')
    native, scratch = {}, Path(scratch)
    with zipfile.ZipFile(aar) as archive:
        infos = zip_entries(archive)
        names = {i.filename for i in infos}
        libs = sorted(n for n in names if re.fullmatch(r'jni/[^/]+/[^/]+\.so', n))
        require(set(n for n in names if n.endswith('.so')) == set(libs), 'unbound AAR native library location')
        require({n.split('/')[1] for n in libs} == set(ABI) and
                all('jni/' + abi + '/libbox.so' in names for abi in ABI), 'formal AAR must contain all four ABIs')
        require('classes.jar' in names and 'AndroidManifest.xml' in names, 'AAR binding/manifest missing')
        manifest = archive.read('AndroidManifest.xml').decode('utf-8')
        require(re.search(r'minSdkVersion\s*=\s*[\"\']24[\"\']', manifest), 'AAR minimumAPI must be24')
        for name in libs:
            abi = name.split('/')[1]
            raw = archive.read(name)
            klass, machine, arch = ABI[abi]
            require(len(raw) >= 64 and raw[4] == klass and struct.unpack_from('<H', raw, 18)[0] == machine and
                    struct.unpack_from('<H', raw, 16)[0] == 3, 'ELF shared-library ABI differs')
            loads = elf_loads(raw)
            require(all(r['alignment'] >= 16384 and (r['virtualAddress'] - r['offset']) % 16384 == 0 for r in loads), '16KiB PT_LOAD required')
            path = scratch / name.replace('/', '_')
            path.write_bytes(raw)
            dynamic = run([tools['readelf'], '-d', str(path)])
            needed = re.findall(r'Shared library: \[([^]]+)\]', dynamic)
            require(all(n in {'liblog.so', 'libdl.so', 'libm.so', 'libandroid.so', 'libc.so'} or
                        'jni/' + abi + '/' + n in names for n in needed), 'unpackaged Android runtime dependency')
            row = {'bytes': len(raw), 'sha256': sha(raw), 'ptLoad': loads, 'needed': needed}
            if name.endswith('/libbox.so'):
                info = run([tools['go'], 'version', '-m', str(path)])
                actual_id = run([tools['go'], 'tool', 'buildid', str(path)]).strip()
                facts = helper.parse_build_info(info)
                check_buildinfo(facts, value, graph, os_name='android', arch=arch, mode='c-shared',
                                expected_id=expected_id, actual_id=actual_id)
                require(android_api(raw) == 24, 'ELF Android API differs')
                symbols = run([tools['readelf'], '--dyn-syms', '--wide', str(path)])
                exported = {m[1] for m in re.finditer(r'GLOBAL\s+DEFAULT\s+(?!UND\b)\S+\s+(\S+)', symbols)}
                require('JNI_OnLoad' in exported and any(n.startswith('Java_go_Seq_') for n in exported),
                        'actual generated JNI loader/sequence exports missing')
                row.update(goBuildInfo=info, parsedBuildInfo=facts, buildID=actual_id, androidAPI=24,
                           jniExports=sorted(n for n in exported if n.startswith(('Java_', 'JNI_'))))
            native[name] = row
        classes = scratch / 'classes.jar'
        classes.write_bytes(archive.read('classes.jar'))
        interfaces = {}
        for name in ['Libbox', 'CommandServer', 'PlatformInterface', 'ConfigValidationResult', 'ExchangeContext', 'LocalDNSTransport', 'Func']:
            interfaces[name] = run([tools['javap'], '-constants', '-classpath', str(classes), 'io.nekohasekai.libbox.' + name])
        required = {'Libbox': ['newTransientCommandServer(', 'newStrictCommandServer(', 'interfaceUpdateListenerIdentity(',
                    'checkConfigWithResult(', 'java.lang.String version()'],
                    'CommandServer': ['startOrReloadService(', 'public native java.lang.String exportTailscaleStoreRetirement();',
                                      'void wakeNow()', 'void recordScreenState(boolean)'],
                    'PlatformInterface': ['void bindInterfaceControl(int, java.lang.String)'],
                    'ConfigValidationResult': ['getRequestID()', 'getConfigDigest()', 'getContractVersion()', 'getValidation()',
                        'getCleanup()', 'getValidationError()', 'getCleanupError()', 'getTailscaleStoreRetirement()', 'getTailscaleStoreMembership()'],
                    'ExchangeContext': ['void errnoCode(int)', 'void errorCode(int)', 'void onCancel(', 'void rawSuccess(byte[])', 'void success(java.lang.String)'],
                    'LocalDNSTransport': ['void exchange(', 'void lookup(', 'boolean raw()'], 'Func': ['void invoke()']}
        for name, declarations in required.items():
            require(all(d in interfaces[name] for d in declarations), 'generated Java API differs: ' + name)
        require('ConfigValidationContractVersion = "polaris-validation-v1"' in interfaces['Libbox'], 'validation API contract differs')
        for setter in ['setRequestID(', 'setConfigDigest(', 'setValidation(', 'setCleanup(', 'setTailscaleStoreRetirement(', 'setTailscaleStoreMembership(']:
            require(setter not in interfaces['ConfigValidationResult'], 'validation result must be readonly')
        return {'format': 'AAR', 'artifact': file_pin(aar), 'nativeLibraries': native,
                'classes': file_pin(classes), 'javaInterfaces': interfaces,
                'manifestSha256': sha(archive.read('AndroidManifest.xml')), 'minimumAPI': 24}


def tree_inventory(root):
    import os
    root = Path(root)
    require(not root.is_symlink() and root.is_dir(), 'component root redirected/missing')
    root = root.resolve(strict=True)
    rows = []
    for current, dirs, files in os.walk(root, followlinks=False):
        for name in sorted(dirs + files):
            path = Path(current) / name
            row = {'path': path.relative_to(root).as_posix(), 'mode': path.lstat().st_mode & 0o777}
            if path.is_symlink():
                target = os.readlink(path)
                require(not Path(target).is_absolute() and path.resolve(strict=True).is_relative_to(root),
                        'framework link must remain inside the same component')
                row.update(type='symlink', target=target)
            elif path.is_file():
                row.update(type='file', **file_pin(path, allow_empty=True))
            else:
                require(path.is_dir(), 'unsupported component object')
                row['type'] = 'directory'
            rows.append(row)
    require(rows, 'empty component tree')
    return sorted(rows, key=lambda row: row['path'])


def framework_entry_targets(entry):
    require(entry.get('SupportedPlatform') == 'ios' and
            entry.get('SupportedPlatformVariant') in (None, 'simulator'), 'framework must contain only device/simulator iOS')
    architectures = entry.get('SupportedArchitectures')
    expected = ['arm64', 'x86_64'] if entry.get('SupportedPlatformVariant') == 'simulator' else ['arm64']
    require(isinstance(architectures, list) and all(isinstance(arch, str) for arch in architectures) and
            len(architectures) == len(expected) and set(architectures) == set(expected),
            'framework declared architecture inventory differs')
    return ({'ios-arm64-simulator', 'ios-x86_64-simulator'} if len(expected) == 2 else {'ios-arm64'})


def objc_validation_getter_contract(header):
    # Actual pinned gobind headers use getTailscaleStore* selectors on this
    # result receiver, not lowercase tailscaleStore* aliases. Comments or
    # selectors on another class cannot supply the immutable result contract.
    code = re.sub(r'/\*.*?\*/', '', header, flags=re.S)
    code = re.sub(r'//[^\n]*', '', code)
    require(len(re.findall(r'@interface\s+LibboxConfigValidationResult\b', code)) == 1,
            'one generated Objective-C validation result receiver required')
    receivers = re.findall(r'@interface\s+LibboxConfigValidationResult\s*:\s*NSObject\s*'
                           r'<\s*goSeqRefInterface\s*>\s*\{\s*\}(.*?)@end\b', code, re.S)
    require(len(receivers) == 1, 'generated Objective-C validation result receiver differs')
    receiver, declarations = receivers[0], {}
    # Method declarations end at semicolons, not newlines. In particular a
    # same-line duplicate getter or setter must enter the same method table.
    statements = receiver.split(';')
    require(not statements[-1].strip(), 'unterminated Objective-C result declaration')
    methods, properties = {}, []
    for statement in statements[:-1]:
        statement = statement.strip()
        if not statement:
            continue
        if statement.startswith('@property'):
            properties.append(statement)
            continue
        method = re.fullmatch(r'([-+])\s*\(([^()]*)\)\s*([A-Za-z_]\w*)\s*(.*)', statement, re.S)
        require(method is not None, 'unexpected Objective-C result declaration')
        sign, return_type, name, arguments = method.groups()
        methods.setdefault(name, []).append((sign, return_type.strip(), arguments.strip()))
    for selector in ['getCleanup', 'getCleanupError', 'getConfigDigest', 'getContractVersion', 'getRequestID',
                     'getTailscaleStoreMembership', 'getTailscaleStoreRetirement', 'getValidation', 'getValidationError']:
        named = methods.get(selector, [])
        require(len(named) == 1 and named[0][0] == '-' and not named[0][2] and
                re.fullmatch(r'NSString\s*\*\s*_Nonnull', named[0][1]) is not None,
                'generated Objective-C immutable getter differs: ' + selector)
        require('set' + selector[3:] not in methods,
                'generated Objective-C validation result setter: ' + selector)
        field = selector[3:4].lower() + selector[4:]
        require(not any(re.search(r'\b' + field + r'\b', item) for item in properties),
                'generated Objective-C validation result property alias: ' + field)
        declarations[selector] = '- (NSString* _Nonnull)' + selector + ';'
    return {'receiver': 'LibboxConfigValidationResult', 'getters': declarations}


def inspect_framework(framework, value, graph, expected_id):
    validate(value)
    framework = Path(framework)
    helper = module('formal_apple_buildinfo', 'scripts/libbox-patches/android-source.py')
    carrier = module('formal_apple_container', 'scripts/apple-carrier.py')
    inventory = tree_inventory(framework)
    info = plistlib.loads((framework / 'Info.plist').read_bytes())
    libraries, observed = info['AvailableLibraries'], {}
    require(isinstance(libraries, list) and len(libraries) == 2 and
            len({r['LibraryIdentifier'] for r in libraries}) == len(libraries), 'framework device/simulator library inventory differs')
    require({r.get('SupportedPlatformVariant') for r in libraries} == {None, 'simulator'},
            'framework must contain one device and one simulator library')
    for entry in libraries:
        expected_targets = framework_entry_targets(entry)
        entry_targets = set()
        identifier, path = entry['LibraryIdentifier'], entry['LibraryPath']
        require(PurePosixPath(identifier).name == identifier and PurePosixPath(path).name == path and
                entry['SupportedPlatform'] == 'ios' and entry.get('SupportedPlatformVariant') in (None, 'simulator'),
                'framework must contain only device/simulator iOS')
        lib = framework / identifier / path
        require(lib.is_dir() and lib.suffix == '.framework', 'expected a framework component')
        # The pinned generator writes an empty Resources/Info.plist and a
        # versioned static Libbox binary; it has no CFBundleExecutable field.
        plistlib.loads((lib / 'Resources/Info.plist').read_bytes())
        binary = (lib / 'Libbox').resolve(strict=True)
        require(binary.is_relative_to(framework.resolve(strict=True)), 'static binary escaped framework')
        raw = binary.read_bytes()
        require(len(raw) <= MAX_ARTIFACT, 'bounded actual framework binary required')
        try:
            parsed = carrier.container(raw, helper)
        except RuntimeError as error:
            # Retain the actual failing header/declared input in the original
            # log; a lipo command alone does not prove the emitted fat count.
            count = struct.unpack_from('>I', raw, 4)[0] if len(raw) >= 8 and raw.startswith(carrier.FAT) else None
            raise RuntimeError('actual framework input refused: path=' + str(binary) +
                '; header=' + raw[:16].hex() + '; fatCount=' + str(count) +
                '; declaredArchitectures=' + repr(entry['SupportedArchitectures']) + '; ' + str(error)) from error
        slices = parsed['slices'] if parsed['kind'] == 'fat-ar' else [{'ordinal': 0, 'archive': parsed}]
        require(len(slices) == len(expected_targets) and all(s['archive']['kind'] == 'bsd-ar' for s in slices),
                'Libbox static slice inventory differs from framework declaration')
        headers = {p.name: p.read_text() for p in (lib / 'Headers').glob('*.h')}
        header = '\n'.join(headers.values())
        for symbol in ['NewTransientCommandServer', 'NewStrictCommandServer', 'InterfaceUpdateListenerIdentity',
                       'CheckConfigWithResult', 'ConfigValidationContractVersion', 'exportTailscaleStoreRetirement',
                       'getTailscaleStoreRetirement', 'getTailscaleStoreMembership', 'wakeNow', 'recordScreenState']:
            require(symbol in header, 'generated Objective-C API missing: ' + symbol)
        validation_getters = objc_validation_getter_contract(header)
        for slice_row in slices:
            members = [m for m in slice_row['archive']['members'] if m['classification'] == 'code']
            require(members, 'static archive has no code')
            cpu = members[0]['machO']['cpu']
            target = next((key for key, spec in APPLE.items() if spec[0] == cpu and spec[3] == entry.get('SupportedPlatformVariant')), None)
            require(target in expected_targets and target not in entry_targets and target not in observed,
                    'unknown/duplicate Apple architecture slice')
            entry_targets.add(target)
            spec = APPLE[target]
            require(all(m['machO']['cpu'] == cpu for m in members), 'mixed archive CPU')
            infos, ids = [], []
            for member in members:
                obj = member['machO']
                go_carrier = any('buildInfo' in s for s in obj['sections'])
                require((not go_carrier or obj['platformCommands']) and
                        all(p['platform'] == spec[1] and 0 < p['minOSRaw'] <= 15 << 16 for p in obj['platformCommands']) and
                        (not go_carrier or all(p['minOSRaw'] == 15 << 16 for p in obj['platformCommands'])),
                        'Apple archive deployment/platform differs')
                for section in obj['sections']:
                    if 'buildInfo' in section:
                        infos.append(section['buildInfo'])
                    ids.extend(n['value'] for n in section.get('buildIDs', []))
            require(len(infos) == len(ids) == 1, 'one original Go carrier per Apple slice required')
            check_buildinfo(infos[0]['parsedBuildInfo'], value, graph, os_name='ios', arch=spec[2], mode='c-archive',
                            expected_id=expected_id, actual_id=ids[0], simulator=spec[3] == 'simulator')
            observed[target] = {'libraryIdentifier': identifier, 'binary': file_pin(binary),
                'headerSha256': {name: sha(text.encode()) for name, text in headers.items()},
                'validationResultGetterContract': validation_getters,
                'goBuildInfo': infos[0], 'buildID': ids[0],
                'linkerOptions': [{'memberOrdinal': m['ordinal'], 'memberName': m['name'],
                                  'memberSha256': m['sha256'], 'command': command}
                                 for m in members for command in m['machO']['loadCommands']
                                 if command['command'] == 0x2d],
                'members': [{**{k: v for k, v in m.items() if k != 'machO'},
                             'goCarrier': any('buildInfo' in s for s in m['machO']['sections'])} for m in members]}
        require(entry_targets == expected_targets, 'actual framework target inventory differs')
    require(set(observed) == set(APPLE), 'formal Apple inventory must contain exactly three architecture slices')
    return {'format': 'static-XCFramework', 'inventory': inventory,
            'inventorySha256': sha(canonical(inventory)), 'slices': observed}
