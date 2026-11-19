"""One content predicate for fresh mobile outputs and their cached consumers."""
import importlib.util
import json
from pathlib import Path, PurePosixPath
import plistlib
import re
import struct
import sys
import tempfile
import hashlib
import math
import zipfile
import time
from contextlib import contextmanager

from fork_input import ROOT, canonical, decode, require, sha, validate

ABI = {'arm64-v8a': 'arm64', 'armeabi-v7a': 'arm', 'x86': '386', 'x86_64': 'amd64'}
APPLE = {'ios-arm64': (0x100000c, 2, 'arm64', None),
         'ios-arm64-simulator': (0x100000c, 7, 'arm64', 'simulator')}
RECEIPT_POLICY = decode(Path(__file__).with_name('receipt-policy.json').read_bytes())
_RECEIPT_LIMITS = RECEIPT_POLICY['receipt']
MAX_ARTIFACT = _RECEIPT_LIMITS['artifactBytes']
MAX_RECEIPT = _RECEIPT_LIMITS['controlBytes']
# Normalized origins remove the measured member x candidate expansion;
# preserve a finite envelope, not the rejected 256MiB capacity workaround.
MAX_APPLE_RECEIPT = _RECEIPT_LIMITS['appleComponentBytes']
MAX_APPLE_LINK_RECEIPT = _RECEIPT_LIMITS['appleLinkBytes']
MAX_JSON_NODES = _RECEIPT_LIMITS['nodes']
MAX_JSON_DEPTH = _RECEIPT_LIMITS['depth']
MAX_JSON_STRING = _RECEIPT_LIMITS['stringChars']


def receipt_shape(value, maximum):
    """Bound the object and individual encoder tokens before serializing."""
    nodes, characters, active = 0, 0, set()
    def walk(item, depth):
        nonlocal nodes, characters
        nodes += 1
        require(nodes <= MAX_JSON_NODES and depth <= MAX_JSON_DEPTH, 'receipt JSON node/depth budget')
        if isinstance(item, str):
            characters += len(item)
            require(len(item) <= MAX_JSON_STRING and characters <= maximum, 'receipt JSON string budget')
        elif isinstance(item, (dict, list, tuple)):
            require(id(item) not in active, 'cyclic receipt JSON')
            active.add(id(item))
            if isinstance(item, dict):
                for key, child in item.items():
                    require(isinstance(key, str) and len(key) <= _RECEIPT_LIMITS['keyChars'], 'receipt JSON key budget')
                    walk(key, depth + 1); walk(child, depth + 1)
            else:
                for child in item: walk(child, depth + 1)
            active.remove(id(item))
        else:
            require(item is None or isinstance(item, (bool, int, float)), 'unsupported receipt JSON value')
            require(not isinstance(item, int) or item.bit_length() <= _RECEIPT_LIMITS['integerBits'], 'receipt JSON integer budget')
            require(not isinstance(item, float) or math.isfinite(item), 'nonfinite receipt JSON')
    walk(value, 0)


def _receipt_chunks(value, maximum, pretty=False):
    receipt_shape(value, maximum)
    encoder = json.JSONEncoder(sort_keys=True, ensure_ascii=pretty, allow_nan=False,
                               indent=2 if pretty else None, separators=None if pretty else (',', ':'))
    total = 0
    for text in encoder.iterencode(value):
        raw = text.encode()
        total += len(raw)
        require(total <= maximum, 'receipt JSON byte budget')
        yield raw
    if pretty:
        require(total + 1 <= maximum, 'receipt JSON byte budget')
        yield b'\n'


def receipt_chunks(value, *, pretty=False):
    yield from _receipt_chunks(value, receipt_limit(value), pretty)


def receipt_field_bytes(value):
    maximum = receipt_limit(value)
    return {key: sum(len(raw) for raw in _receipt_chunks(child, maximum)) for key, child in value.items()}


def receipt_digest(value):
    digest = hashlib.sha256()
    for raw in receipt_chunks(value): digest.update(raw)
    return digest.hexdigest()


def receipt_lexical_budget(raw):
    # Scan bytes without creating decoded strings/containers. Bound potential
    # DOM allocation before json.loads; decode still validates actual syntax,
    # duplicate keys and nonfinite values, then the object receives shape checks.
    quoted = escaped = False
    depth = tokens = string_bytes = 0
    for byte in raw:
        if quoted:
            string_bytes += 1
            require(string_bytes <= 6 * MAX_JSON_STRING + 2, 'receipt JSON encoded string budget')
            if escaped: escaped = False
            elif byte == 92: escaped = True
            elif byte == 34: quoted = False
        elif byte == 34:
            quoted = True; string_bytes = 0; tokens += 1
        elif byte in (123, 91):
            depth += 1; tokens += 1
            require(depth <= MAX_JSON_DEPTH, 'receipt JSON depth budget')
        elif byte in (125, 93): depth -= 1
        elif byte in (44, 58): tokens += 1
        require(tokens <= 3 * MAX_JSON_NODES, 'receipt JSON lexical node budget')


def receipt_limit(value):
    # Full Apple member/origin inventories can exceed the small-control
    # envelope. Keep those original fields, with a separate finite budget.
    require(isinstance(value, dict), 'receipt must be an object')
    if value.get('schema') == 'polaris-mobile-component-v1' and value.get('platform') == 'apple':
        return MAX_APPLE_RECEIPT
    if value.get('schema') == 'polaris-mobile-apple-link-input-v1':
        return MAX_APPLE_LINK_RECEIPT
    return MAX_RECEIPT


def read_receipt(path):
    path = Path(path)
    size = path.lstat().st_size if path.exists() or path.is_symlink() else 0
    print(json.dumps({'schema': 'polaris-receipt-size-observation-v1', 'phase': 'read',
                      'path': str(path), 'bytes': size, 'symlink': path.is_symlink(),
                      'maximumReadBytes': MAX_APPLE_RECEIPT,
                      'scope': 'actual lstat size; not receipt or carrier acceptance'}), file=sys.stderr, flush=True)
    require(path.is_file() and not path.is_symlink() and 0 < size <= MAX_APPLE_RECEIPT,
            'bounded regular original receipt required: path=' + str(path) + '; bytes=' + str(size) +
            '; maximum=' + str(MAX_APPLE_RECEIPT))
    with path.open('rb') as stream:
        raw = stream.read(MAX_APPLE_RECEIPT + 1)
    require(len(raw) == size, 'receipt size changed during read')
    receipt_lexical_budget(raw)
    value = decode(raw, maximum=MAX_APPLE_RECEIPT)
    require(len(raw) <= receipt_limit(value), 'original receipt exceeds its schema budget')
    receipt_shape(value, receipt_limit(value))
    return value


def module(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


@contextmanager
def inspection_stage(stage, reads=None):
    """Ephemeral timing/read observations; never receipt or cache identity."""
    started, primary = time.monotonic(), None
    try:
        yield
    except BaseException as error:
        # Only a failure raised by this operation is primary. An already
        # handled exception in the caller must not suppress observer failure.
        primary = error
        raise
    finally:
        try:
            # Lazy import avoids artifacts/producer's existing import cycle.
            # The explicit borrowed descriptor crosses the outer capture layer;
            # no native tool text or path is forwarded by this observation.
            from producer import inspection_progress
            inspection_progress(stage, time.monotonic() - started,
                                reads.get('fileReads', 0) if reads is not None else None,
                                reads.get('readBytes', 0) if reads is not None else None)
        except BaseException:
            if primary is None:
                # Keep the existing observer-only failure policy.
                raise
            failure_note(primary, 'Apple inspection observation failed; original failure preserved')


def failure_note(error, message):
    """Retain fixed exception metadata on Python 3.10 as well as newer hosts."""
    try:
        notes = error.__dict__.get('__notes__')
        if notes is None:
            error.__dict__['__notes__'] = [message]
        elif isinstance(notes, list):
            notes.append(message)
    except BaseException:
        # A diagnostic annotation must never replace the primary failure.
        pass


def read_pinned(path, *, allow_empty=False, observations=None):
    """Read bounded original bytes once for both content parsing and pinning."""
    path = Path(path)
    require(path.is_file() and not path.is_symlink(), 'bounded regular artifact required')
    size = path.stat().st_size
    require((allow_empty or size > 0) and size <= MAX_ARTIFACT, 'bounded regular artifact required')
    with path.open('rb') as stream:
        raw = stream.read(size + 1)
    require(len(raw) == size and len(raw) <= MAX_ARTIFACT, 'artifact size changed during bounded read')
    if observations is not None:
        observations['fileReads'] = observations.get('fileReads', 0) + 1
        observations['readBytes'] = observations.get('readBytes', 0) + len(raw)
    return raw, {'bytes': len(raw), 'sha256': sha(raw)}


def file_pin(path, *, allow_empty=False, observations=None):
    return read_pinned(path, allow_empty=allow_empty, observations=observations)[1]


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


def component_output_identity(output):
    """Compare content while retaining the original verifier stdout observation.

    Go1.25.5 version.scanFile prints its invocation filename in the first
    line. A consumer uses its own scratch directory for the same pinned ELF.
    Only that absolute filename is observational; the version, entire module
    body, parsed facts, native bytes, BuildID, JNI/API and all other output
    fields still participate in equality. Neither original output is mutated.
    Non-AAR outputs need no normalization and are returned as borrowed, read-only
    comparison values; callers still re-inspect the current original bytes.
    """
    if output.get('format') != 'AAR':
        return output
    result = json.loads(json.dumps(output))
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


def _read_native_buildinfo(evidence, run, scratch):
    raw = b''.join(_receipt_chunks(evidence, MAX_RECEIPT))
    verifier = Path(__file__).with_name('source-admission.mjs').resolve()
    with tempfile.TemporaryDirectory(prefix='native-buildinfo-', dir=scratch) as folder:
        report = Path(folder) / 'observations.json'
        report.write_bytes(raw)
        return json.loads(run(['node', verifier, '--native-buildinfo', report, len(raw), sha(raw)]))


def verify_native_buildinfo(observation, value, graph, *, os_name, arch, mode, expected_id, actual_id,
                            run, scratch, simulator=False):
    evidence = {'raw': observation, 'source': value, 'graph': graph, 'expectedId': expected_id, 'actualId': actual_id,
                'target': {'osName': os_name, 'arch': arch, 'mode': mode, 'simulator': simulator}}
    return _read_native_buildinfo(evidence, run, scratch)


class AppleBuildInfoBatch:
    """Project all raw text from one inspected carrier without crossing its ownership."""

    def __init__(self):
        self.observations = {}
        self.text_bytes = 0

    def observe(self, info):
        raw = info['parserInput']
        self.text_bytes += len(raw.encode())
        require(self.text_bytes <= MAX_RECEIPT, 'Apple BuildInfo batch exceeds control byte budget')
        self.observations[id(info)] = (info, {'raw': raw})

    def bind(self, info, *, arch, simulator, expected_id, actual_id):
        self.observations[id(info)][1].update(
            target={'osName': 'ios', 'arch': arch, 'mode': 'c-archive', 'simulator': simulator},
            expectedId=expected_id, actualId=actual_id)

    def verify(self, *, run, scratch, source=None, graph=None):
        if not self.observations:
            return
        evidence = {'appleObservations': [row for _, row in self.observations.values()]}
        if source is not None:
            evidence.update(source=source, graph=graph)
        projection = _read_native_buildinfo(evidence, run, scratch)
        for (info, _), facts in zip(self.observations.values(), projection, strict=True):
            info['parsedBuildInfo'] = facts
            # This field predates the UTF-8 receipt digest and uses ASCII JSON.
            info['buildInfoFingerprint'] = sha(json.dumps(facts, sort_keys=True, separators=(',', ':'),
                                                          ensure_ascii=True).encode())


def inspect_aar(aar, value, graph, tools, run, scratch, expected_id):
    validate(value)
    verifier = ROOT / 'scripts/verify-apk.mjs'
    native, scratch = {}, Path(scratch)
    jni = {}
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
            libbox = name.endswith('/libbox.so')
            raw = archive.read(name)
            size, digest = len(raw), sha(raw)
            path = scratch / name.replace('/', '_')
            path.write_bytes(raw)
            del raw
            elf = json.loads(run(['node', verifier, '--aar-elf', path,
                                  abi, size, digest, 'libbox' if libbox else 'dependency']))
            dynamic = run([tools['readelf'], '-d', str(path)])
            needed = re.findall(r'Shared library: \[([^]]+)\]', dynamic)
            require(all(n in {'liblog.so', 'libdl.so', 'libm.so', 'libandroid.so', 'libc.so'} or
                        'jni/' + abi + '/' + n in names for n in needed), 'unpackaged Android runtime dependency')
            row = {'bytes': size, 'sha256': digest, 'ptLoad': elf['ptLoad'], 'needed': needed}
            if libbox:
                info = run([tools['go'], 'version', '-m', str(path)])
                actual_id = run([tools['go'], 'tool', 'buildid', str(path)]).strip()
                facts = verify_native_buildinfo(info, value, graph, os_name='android', arch=ABI[abi], mode='c-shared',
                                                expected_id=expected_id, actual_id=actual_id, run=run, scratch=scratch)
                jni[abi] = run([tools['readelf'], '--dyn-syms', '--wide', str(path)])
                row.update(goBuildInfo=info, parsedBuildInfo=facts, buildID=actual_id, androidAPI=elf['androidAPI'])
            native[name] = row
        classes = scratch / 'classes.jar'
        classes.write_bytes(archive.read('classes.jar'))
        plan = json.loads(run(['node', verifier, '--aar-interface-classes']))
        observations = {'jni': jni}
        for field, option in [('hotJavaInterfaces', '-s'), ('javaInterfaces', '-constants')]:
            observations[field] = {
                name: run([tools['javap'], option, '-classpath', str(classes), 'io.nekohasekai.libbox.' + name])
                for name in plan[field]
            }
        raw = canonical(observations)
        require(len(raw) <= MAX_RECEIPT, 'AAR interface observations exceed fixed control budget')
        with tempfile.TemporaryDirectory(prefix='aar-interfaces-', dir=scratch) as folder:
            report = Path(folder) / 'observations.json'
            report.write_bytes(raw)
            exports = json.loads(run(['node', verifier, '--aar-interfaces', report, len(raw), sha(raw)]))
        for abi, symbols in exports.items():
            native['jni/' + abi + '/libbox.so']['jniExports'] = symbols
        return {'format': 'AAR', 'artifact': file_pin(aar), 'nativeLibraries': native,
                'classes': file_pin(classes), 'javaInterfaces': observations['javaInterfaces'],
                'hotJavaInterfaces': observations['hotJavaInterfaces'],
                'manifestSha256': sha(archive.read('AndroidManifest.xml')), 'minimumAPI': 24}


def tree_inventory(root, *, pinned=None, observations=None, metadata_only=False):
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
                row['type'] = 'file'
                if not metadata_only:
                    key = path.resolve(strict=True)
                    pin = pinned.get(key) if pinned is not None else None
                    row.update(pin if pin is not None else
                               file_pin(path, allow_empty=True, observations=observations))
            else:
                require(path.is_dir(), 'unsupported component object')
                row['type'] = 'directory'
            rows.append(row)
    require(rows, 'empty component tree')
    return sorted(rows, key=lambda row: row['path'])


def _inspect_apple_sdk(mode, observation, run, scratch):
    with tempfile.TemporaryDirectory(prefix='apple-sdk-', dir=scratch) as folder:
        report = Path(folder) / 'observation'
        report.write_bytes(observation)
        return json.loads(run(['node', ROOT / 'scripts/mobile-core/apple-admission.mjs',
                               mode, report.resolve(), len(observation), sha(observation)]))


def inspect_framework(framework, value, graph, expected_id, *, run, scratch):
    reads = {}
    with inspection_stage('framework-inspection', reads):
        return _inspect_framework(framework, value, graph, expected_id, reads, run, scratch)


def _inspect_framework(framework, value, graph, expected_id, reads, run, scratch):
    validate(value)
    framework = Path(framework)
    carrier = module('formal_apple_container', 'scripts/apple-carrier.py')
    buildinfo = AppleBuildInfoBatch()
    # Pins used by parsing are reused only within this invocation. A complete
    # fresh inventory at the final boundary detects changes to bytes/modes,
    # links and the file set without retaining multiple large binary buffers.
    layout = tree_inventory(framework, metadata_only=True)
    pinned = {}
    raw_info, info_pin = read_pinned(framework / 'Info.plist', observations=reads)
    pinned[(framework / 'Info.plist').resolve(strict=True)] = info_pin
    libraries = plistlib.loads(raw_info)['AvailableLibraries']
    if isinstance(libraries, list):
        fields = ('LibraryIdentifier', 'LibraryPath', 'SupportedPlatform', 'SupportedPlatformVariant', 'SupportedArchitectures')
        libraries = [{key: entry.get(key) for key in fields} if isinstance(entry, dict) else None for entry in libraries]
    declared = _inspect_apple_sdk('--sdk-framework-libraries', b''.join(_receipt_chunks(libraries, MAX_RECEIPT)), run, scratch)
    observed = {}
    for entry in declared:
        identifier, path, target = entry['identifier'], entry['path'], entry['target']
        spec = APPLE[target]
        lib = framework / identifier / path
        require(lib.is_dir() and lib.suffix == '.framework', 'expected a framework component')
        # The pinned generator writes an empty Resources/Info.plist and a
        # versioned static Libbox binary; it has no CFBundleExecutable field.
        resource_info = lib / 'Resources/Info.plist'
        resource_raw, resource_pin = read_pinned(resource_info, observations=reads)
        pinned[resource_info.resolve(strict=True)] = resource_pin
        plistlib.loads(resource_raw)
        binary = (lib / 'Libbox').resolve(strict=True)
        require(binary.is_relative_to(framework.resolve(strict=True)), 'static binary escaped framework')
        raw, binary_pin = read_pinned(binary, observations=reads)
        pinned[binary] = binary_pin
        try:
            parsed = carrier.container(raw, buildinfo.observe)
        except RuntimeError as error:
            # Retain the actual failing header/declared input in the original
            # log; a lipo command alone does not prove the emitted fat count.
            count = struct.unpack_from('>I', raw, 4)[0] if len(raw) >= 8 and raw.startswith(carrier.FAT) else None
            raise RuntimeError('actual framework input refused: path=' + str(binary) +
                '; header=' + raw[:16].hex() + '; fatCount=' + str(count) +
                '; declaredArchitectures=' + repr(entry['architectures']) + '; ' + str(error)) from error
        slices = parsed['slices'] if parsed['kind'] == 'fat-ar' else [{'ordinal': 0, 'archive': parsed}]
        require(len(slices) == 1 and slices[0]['archive']['kind'] == 'bsd-ar',
                'Libbox static slice inventory differs from framework declaration')
        headers = {}
        for header_path in (lib / 'Headers').glob('*.h'):
            resolved = header_path.resolve(strict=True)
            require(resolved.is_relative_to(framework.resolve(strict=True)), 'header escaped framework')
            header_raw, header_pin = read_pinned(resolved, observations=reads)
            pinned[resolved] = header_pin
            # Match read_text's universal newline behavior in the stable output.
            headers[header_path.name] = header_raw.decode().replace('\r\n', '\n').replace('\r', '\n')
        # Keep the exact text observed from the pinned framework files, in
        # their original order and normalized as in the receipt.
        header = '\n'.join(headers.values()).encode()
        contracts = _inspect_apple_sdk('--sdk-header-contract', header, run, scratch)
        for slice_row in slices:
            members = [m for m in slice_row['archive']['members'] if m['classification'] == 'code']
            require(members, 'static archive has no code')
            cpu = members[0]['machO']['cpu']
            require(cpu == spec[0] and target not in observed, 'unknown/duplicate Apple architecture slice')
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
            buildinfo.bind(infos[0], arch=spec[2], simulator=spec[3] == 'simulator',
                           expected_id=expected_id, actual_id=ids[0])
            observed[target] = {'libraryIdentifier': identifier, 'binary': binary_pin,
                'headerSha256': {name: sha(text.encode()) for name, text in headers.items()},
                'validationResultGetterContract': contracts['validationResultGetterContract'],
                'hotObjectiveCContract': contracts['hotObjectiveCContract'],
                'goBuildInfo': infos[0], 'buildID': ids[0],
                'linkerOptions': [{'memberOrdinal': m['ordinal'], 'memberName': m['name'],
                                  'memberSha256': m['sha256'], 'command': command}
                                 for m in members for command in m['machO']['loadCommands']
                                 if command['command'] == 0x2d],
                'members': [{**{k: v for k, v in m.items() if k != 'machO'},
                             'goCarrier': any('buildInfo' in s for s in m['machO']['sections'])} for m in members]}
    require(set(observed) == set(APPLE), 'formal Apple inventory must contain exactly two arm64 platform slices')
    buildinfo.verify(run=run, scratch=scratch, source=value, graph=graph)
    inventory = tree_inventory(framework, pinned=pinned, observations=reads)
    require([{k: v for k, v in row.items() if k not in ('bytes', 'sha256')} for row in inventory] == layout,
            'framework layout changed during original-byte inspection')
    require(tree_inventory(framework, observations=reads) == inventory,
            'framework changed during original-byte inspection')
    return {'format': 'static-XCFramework', 'inventory': inventory,
            'inventorySha256': sha(canonical(inventory)), 'slices': observed}
