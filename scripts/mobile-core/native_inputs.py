"""Retain actual compiler/static origins; cache validation rereads original bytes."""
from collections import Counter, defaultdict
from pathlib import Path
import re
import shlex

from fork_input import require
from artifacts import file_pin, module, tree_inventory, read_pinned


def source_table(files, *, reader=None):
    """Build origin facts from local originals or a bounded archive reader."""
    reader = read_pinned if reader is None else reader
    require(0 < len(files) <= 65536, 'native source file count budget')
    helper = module('origin_buildinfo', 'scripts/libbox-patches/android-source.py')
    carrier = module('origin_container', 'scripts/apple-carrier.py')
    origins = defaultdict(list)
    rows = characters = 0
    def append(digest, row):
        nonlocal rows, characters
        rows += 1; characters += len(row['path'])
        require(rows <= 65536 and len(row['path']) <= 4096 and characters <= 16 * 1024 ** 2,
                'native source table row/path budget')
        origins[digest].append(row)
    for path, pin in sorted(files.items()):
        if pin['kind'] == 'compiler-object':
            append(pin['sha256'], {'kind': pin['kind'], 'path': path, 'sha256': pin['sha256']})
        else:
            require(pin['kind'] == 'external-static-archive', 'unknown native origin kind')
            raw, actual_pin = reader(path)
            require(actual_pin == {key: pin[key] for key in ('bytes', 'sha256')},
                    'static origin changed during inspection')
            parsed = carrier.container(raw, helper)
            slices = parsed['slices'] if parsed['kind'] == 'fat-ar' else [{'ordinal': 0, 'archive': parsed}]
            for row in slices:
                require(row['archive']['kind'] == 'bsd-ar', 'external runtime must be a static archive')
                for member in row['archive']['members']:
                    if member['classification'] == 'code':
                        append(member['sha256'], {'kind': pin['kind'], 'path': path,
                            'archiveSha256': pin['sha256'], 'sliceOrdinal': row['ordinal'],
                            'memberOrdinal': member['ordinal'], 'sha256': member['sha256']})
    return dict(origins)


def reference_layout(output):
    require(isinstance(output, dict) and output.get('format') == 'static-XCFramework',
            'actual static output required for origins')
    require(isinstance(output.get('slices'), dict) and 0 < len(output['slices']) <= 65536,
            'actual static slices required for origins')
    mappings, count = {}, 0
    for target, row in output['slices'].items():
        require(isinstance(target, str) and 0 < len(target) <= 4096 and isinstance(row, dict)
                and isinstance(row.get('members'), list), 'invalid static target/member layout')
        mappings[target] = []
        ordinals = set()
        for member in row['members']:
            count += 1
            require(count <= 65536, 'static member reference count budget')
            require(isinstance(member, dict), 'invalid static member layout')
            ordinal = member.get('ordinal')
            require(type(ordinal) is int and ordinal >= 0 and ordinal not in ordinals, 'duplicate/invalid static member ordinal')
            ordinals.add(ordinal)
            require(type(member.get('goCarrier')) is bool and isinstance(member.get('sha256'), str)
                    and re.fullmatch('[0-9a-f]{64}', member['sha256']), 'invalid static member digest/classification')
            if member['goCarrier']: continue
            mappings[target].append({'memberOrdinal': ordinal, 'sha256': member['sha256']})
    return mappings


def bind_origins(mappings, origins):
    used_digests = set()
    for rows in mappings.values():
        used = Counter(row['sha256'] for row in rows)
        for digest, count in used.items():
            require(origins.get(digest) and count <= len(origins[digest]),
                    'static member lacks actual compiler/runtime origin')
            used_digests.add(digest)
    return {digest: origins[digest] for digest in sorted(used_digests)}


def member_references(output, origins):
    mappings = reference_layout(output)
    return mappings, bind_origins(mappings, origins)


def origin_identity(row):
    """One bounded, hashable identity; never sort/copy a full origins list."""
    require(isinstance(row, dict), 'invalid legacy/static origin identity')
    keys = {'kind', 'path', 'sha256'}
    archive = row.get('kind') == 'external-static-archive'
    if archive: keys |= {'archiveSha256', 'sliceOrdinal', 'memberOrdinal'}
    require(set(row) == keys and row.get('kind') in ('compiler-object', 'external-static-archive')
            and isinstance(row.get('path'), str) and 0 < len(row['path']) <= 4096
            and isinstance(row.get('sha256'), str) and re.fullmatch('[0-9a-f]{64}', row['sha256']),
            'invalid legacy/static origin identity')
    identity = (row['kind'], row['path'], row['sha256'])
    if archive:
        require(isinstance(row['archiveSha256'], str) and re.fullmatch('[0-9a-f]{64}', row['archiveSha256'])
                and all(type(row[key]) is int and row[key] >= 0 for key in ('sliceOrdinal', 'memberOrdinal')),
                'invalid legacy/static archive origin identity')
        identity += (row['archiveSha256'], row['sliceOrdinal'], row['memberOrdinal'])
    return identity


def verify_mapping_shape(facts, refs):
    mapping = facts.get('staticMemberOrigins')
    require(isinstance(mapping, dict) and mapping.keys() == refs.keys(),
            'static source table/member targets required')
    legacy = facts['schema'] == 'polaris-mobile-native-inputs-v1'
    keys = {'memberOrdinal', 'sha256'} | ({'origins'} if legacy else set())
    for target, wanted in refs.items():
        rows = mapping[target]
        require(isinstance(rows, list) and len(rows) == len(wanted), 'static source table/member counts differ')
        for row, reference in zip(rows, wanted):
            require(isinstance(row, dict) and set(row) == keys and type(row.get('memberOrdinal')) is int
                    and row['memberOrdinal'] == reference['memberOrdinal'] and row.get('sha256') == reference['sha256'],
                    'static source table/member references differ')
            if legacy:
                require(isinstance(row['origins'], list) and 0 < len(row['origins']) <= 65536,
                        'legacy actual source/member origins required')
    if not legacy:
        require(isinstance(facts.get('originsByDigest'), dict), 'actual source table/member origins required')


def collect(scratch, log, platform, output, cache):
    scratch, cache = Path(scratch).resolve(strict=True), Path(cache).resolve(strict=True)
    text = Path(log).read_text()
    roots = sorted({Path(p).resolve(strict=True) for p in re.findall(r'^WORK=(.+)$', text, re.M)})
    require(roots and len(roots) <= 32 and all(p.is_dir() and p.is_relative_to(scratch) for p in roots),
            'actual compiler WORK roots must remain in the fixed scratch')
    files = {}
    for root in roots:
        for p in root.rglob('*.o'):
            require(not p.is_symlink() and p.resolve().is_relative_to(scratch), 'compiler object escaped scratch')
            files[str(p)] = {'kind': 'compiler-object', **file_pin(p)}
            require(len(files) <= 65536, 'native source file count budget')
    # The pinned generator prints its merge commands with a $WORK alias.
    # The gomobile workspace is the one containing generated binding sources.
    mobile = [p for p in roots if any((p / target).is_dir() for target in ('ios', 'iossimulator', 'src'))]
    if platform == 'apple':
        require(len(mobile) == 1, 'one actual gomobile workspace required')
        for line in text.splitlines():
            if 'libtool -static -o ' not in line:
                continue
            tokens = shlex.split(line.replace('$WORK', str(mobile[0])).replace('$GOPATH', str(cache / 'go-path'))
                                .replace('$GOMOBILE', str(cache / 'go-path/pkg/gomobile')))
            start = tokens.index('-o') + 3  # output, original Go archive, then external archives
            for token in tokens[start:]:
                p = Path(token).resolve(strict=True)
                require(p.suffix == '.a' and p.is_relative_to(cache), 'external static origin outside verified module cache')
                files[str(p)] = {'kind': 'external-static-archive', **file_pin(p)}
                require(len(files) <= 65536, 'native source file count budget')
    require(files, 'actual compiler/static input bytes are missing')
    facts = {'schema': 'polaris-mobile-native-inputs-v1', 'scratch': str(scratch), 'cache': str(cache),
             'workRoots': [str(p) for p in roots], 'originalBindLog': file_pin(log), 'files': files}
    if platform == 'apple':
        mappings, origins = member_references(output, source_table(files))
        facts.update(schema='polaris-mobile-native-inputs-v2', staticMemberOrigins=mappings, originsByDigest=origins)
    return facts


def verify(facts, scratch=None, log=None, output=None):
    require(facts.get('schema') in ('polaris-mobile-native-inputs-v1', 'polaris-mobile-native-inputs-v2'), 'actual native input receipt required')
    static = (isinstance(output, dict) and output.get('format') == 'static-XCFramework')
    static = static or facts['schema'] == 'polaris-mobile-native-inputs-v2' or 'staticMemberOrigins' in facts
    if static:
        refs = reference_layout(output)
        # Static output requires the mapping even when a v1 receipt omits it.
        # Reject wrong targets/references/counts before inspecting source archives.
        verify_mapping_shape(facts, refs)
    if 'generatorOutputRoot' in facts:
        generated = Path(facts['generatorOutputRoot']).resolve(strict=True)
        require(generated.name == 'build' and generated.parent.name == 'build-view',
                'generator output escaped the fixed build view')
        require(tree_inventory(generated) == facts['generatorOutputInventory'],
                'original generator output bytes differ')
    original_scratch, cache = Path(facts['scratch']).resolve(strict=True), Path(facts['cache']).resolve(strict=True)
    if scratch is not None:
        require(original_scratch == Path(scratch).resolve(strict=True), 'native scratch input changed')
    if log is not None:
        require(file_pin(log) == facts['originalBindLog'], 'original compiler log changed')
    require(0 < len(facts['files']) <= 65536, 'native input inventory bounds')
    for path, pin in facts['files'].items():
        p = Path(path)
        require(not p.is_symlink() and p.resolve(strict=True).is_relative_to(
                original_scratch if pin['kind'] == 'compiler-object' else cache), 'native input escaped owned roots')
        # The external archive's same bounded read below verifies its pin and
        # parses members; avoid a second full read at this single boundary.
        if not (static and pin['kind'] == 'external-static-archive'):
            require(file_pin(p) == {key: pin[key] for key in ('bytes', 'sha256')}, 'actual native input bytes changed')
    if static:
        actual = bind_origins(refs, source_table(facts['files']))
        if facts['schema'] == 'polaris-mobile-native-inputs-v2':
            require(facts.get('originsByDigest') == actual and facts.get('staticMemberOrigins') == refs,
                    'actual source table/member references/counts differ')
            for choices in facts['originsByDigest'].values():
                require(isinstance(choices, list), 'actual source table/member origins required')
                for origin in choices: origin_identity(origin)
        else:
            # Normalize each actual digest's complete candidates only once. The
            # historical inline input is already expanded; scan each old list
            # with one bounded counter, without an expanded expected mapping.
            normalized = {digest: Counter(origin_identity(row) for row in choices)
                          for digest, choices in actual.items()}
            for rows in facts['staticMemberOrigins'].values():
                for row in rows:
                    digest = row['sha256']; expected = normalized[digest]
                    require(len(row['origins']) == len(actual[digest]), 'legacy actual source/member origin counts differ')
                    seen = Counter()
                    for origin in row['origins']:
                        key = origin_identity(origin)
                        seen[key] += 1
                        require(seen[key] <= expected.get(key, 0), 'legacy actual source/member origins differ')
