"""Exact reviewed source facts; no candidate selection, build or publication."""
import hashlib
import json
import os
from pathlib import Path
import re


# Names and source-owned paths are schema, not mutable source selections. Tree,
# version and byte facts belong to the explicitly reviewed caller declaration.
VENDOR_DIRECTORIES = {
    'github.com/sagernet/sing-tun': 'sing-tun',
    'github.com/sagernet/tailscale': 'tailscale',
    'github.com/metacubex/utls': 'metacubex-utls',
    'github.com/sagernet/cronet-go': 'cronet-go',
    'github.com/sagernet/fswatch': 'fswatch',
    'github.com/sagernet/sing': 'sing',
    'github.com/sagernet/sing-quic': 'sing-quic',
    'github.com/sagernet/wireguard-go': 'wireguard-go',
    'github.com/database64128/tfo-go/v2': 'tfo-go',
}


VENDOR_DIRECTORIES_V3 = {**VENDOR_DIRECTORIES, 'github.com/sagernet/sing-vmess': 'sing-vmess'}
VENDOR_SCHEMAS = {
    'polaris-reviewed-kernel-source-v2': VENDOR_DIRECTORIES,
    'polaris-reviewed-kernel-source-v3': VENDOR_DIRECTORIES_V3,
}


def vendor_directories(schema):
    require(schema in VENDOR_SCHEMAS, 'explicit reviewed vendor schema required')
    return VENDOR_SCHEMAS[schema]


def require(condition, reason):
    if not condition:
        raise RuntimeError('reviewed source policy: ' + reason)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False,
                      allow_nan=False).encode()


def hexadecimal(value, width):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{%d}' % width, value) is not None


def validate_source_policy(policy, context):
    fields = {'schema', 'sourceCommit', 'sourceTree', 'goDirective', 'goVersion', 'toolchain',
              'goModSha256', 'goSumSha256', 'metacubex'}
    require(isinstance(policy, dict), 'complete explicit declaration required')
    multi_vendor = policy.get('schema') in VENDOR_SCHEMAS
    require(set(policy) == fields | ({'vendors'} if multi_vendor else set()) and
            policy['schema'] in ('polaris-reviewed-kernel-source-v1', *VENDOR_SCHEMAS),
            'complete explicit declaration required')
    for key in ('sourceCommit', 'sourceTree'):
        require(hexadecimal(policy[key], 40) and policy[key] == context.get(key), key + ' differs')
    require(policy['goDirective'] == '1.27.0' and policy['goVersion'] == '1.27.2' and
            policy['toolchain'] == 'go1.27.2', 'exact Go directive/toolchain required')
    for key in ('goModSha256', 'goSumSha256'):
        require(hexadecimal(policy[key], 64) and
                (key not in context or context[key] == policy[key]), key + ' differs')
    meta = policy['metacubex']
    fields = {'module', 'version', 'replacementPath', 'replacementTree', 'replacementFileCount',
              'bindingPath', 'bindingSha256', 'patchPath', 'patchSha256',
              'upstreamCommit', 'upstreamH1', 'upstreamZipSha256'}
    require(isinstance(meta, dict) and set(meta) == fields and
            meta['module'] == 'github.com/metacubex/utls' and meta['version'] == 'v1.8.8' and
            meta['replacementPath'] == './third_party/metacubex-utls' and
            type(meta['replacementFileCount']) is int and meta['replacementFileCount'] == 355 and
            meta['bindingPath'] == 'release/dependency-sources/metacubex-utls-replacement.json' and
            meta['patchPath'] == 'release/dependency-sources/metacubex-utls-reality-alpn.patch',
            'exact Meta source declaration required')
    require(hexadecimal(meta['replacementTree'], 40) and hexadecimal(meta['upstreamCommit'], 40) and
            all(hexadecimal(meta[key], 64) for key in ('bindingSha256', 'patchSha256', 'upstreamZipSha256')) and
            isinstance(meta['upstreamH1'], str) and re.fullmatch(r'h1:[A-Za-z0-9+/]{43}=', meta['upstreamH1']),
            'Meta source hashes/baseline sum differ')
    if multi_vendor:
        validate_vendors(policy['vendors'], versions=True, schema=policy['schema'])
        require(policy['vendors'][meta['module']] == {key: meta[key] for key in
                ('version', 'replacementPath', 'replacementTree', 'replacementFileCount')},
                'Meta and reviewed-vendor declarations disagree')
    return policy


def validate_vendors(vendors, *, versions=False, schema='polaris-reviewed-kernel-source-v2'):
    directories = vendor_directories(schema)
    require(isinstance(vendors, dict) and set(vendors) == set(directories),
            'exact reviewed vendor names required')
    fields = {'replacementPath', 'replacementTree', 'replacementFileCount'}
    for name, pin in vendors.items():
        require(isinstance(pin, dict) and set(pin) == fields | ({'version'} if versions else set()),
                'complete vendor declaration required: ' + name)
        require(pin['replacementPath'] == './third_party/' + directories[name] and
                hexadecimal(pin['replacementTree'], 40) and type(pin['replacementFileCount']) is int and
                0 < pin['replacementFileCount'] <= 65536, 'invalid vendor pin: ' + name)
        if versions:
            require(isinstance(pin['version'], str) and
                    re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?', pin['version']),
                    'explicit vendor module version required: ' + name)
    return vendors


def candidate_vendors(pins, *, schema='polaris-reviewed-kernel-source-v2'):
    """Adapt the frozen Android/desktop tuple declaration without changing it."""
    directories = vendor_directories(schema)
    require(isinstance(pins, dict) and set(pins) == set(directories), 'exact reviewed vendor names required')
    result = {}
    for name, row in pins.items():
        require(isinstance(row, (tuple, list)) and len(row) == 3 and row[0] == directories[name],
                'invalid kernel vendor pin')
        result[name] = dict(replacementPath='./third_party/' + row[0],
                            replacementTree=row[1], replacementFileCount=row[2])
    return validate_vendors(result, schema=schema)


def verify_vendor_module_declarations(raw, vendors):
    directives, dependencies, replacements = source_module_declarations(raw)
    require(directives == {'module': 'github.com/sagernet/sing-box', 'go': '1.27.0', 'toolchain': 'go1.27.2'},
            'kernel Go directives differ')
    local = {}
    for name, (version, target) in replacements.items():
        if len(target) == 1:
            require(not version, 'version-scoped local replacement refused')
            local[name] = target[0]
    require(local == {name: pin['replacementPath'] for name, pin in vendors.items()},
            'exact reviewed local replacements required')
    for name, pin in vendors.items():
        if 'version' in pin:
            require(dependencies.get(name) == pin['version'], 'actual vendor dependency version differs: ' + name)
    return directives, dependencies, replacements


def git_object(kind, raw):
    return hashlib.sha1(kind + b' ' + str(len(raw)).encode() + b'\0' + raw).digest()


def vendor_tree(entries):
    """Reconstruct the full Git tree from observed bytes and executable/link modes."""
    root = {}
    for name, (mode, raw) in entries.items():
        parts, node = name.split('/'), root
        for part in parts[:-1]:
            node = node.setdefault(part, {})
            require(isinstance(node, dict), 'vendor file/directory collision')
        require(parts[-1] not in node, 'duplicate vendor tree entry')
        node[parts[-1]] = (mode, raw)
    def tree(node):
        result = []
        for name, value in node.items():
            directory = isinstance(value, dict)
            mode, digest = ('40000', tree(value)) if directory else (value[0], git_object(b'blob', value[1]))
            result.append((name.encode() + (b'/' if directory else b''),
                           mode.encode() + b' ' + name.encode() + b'\0' + digest))
        return git_object(b'tree', b''.join(row for _, row in sorted(result)))
    return tree(root).hex()


def verify_vendor_files(vendors, files, modes, trees):
    """Pure byte/mode proof, shared by formal source and frozen candidate callers."""
    require(isinstance(modes, dict) and isinstance(trees, dict) and set(trees) == set(vendors),
            'complete vendor mode/tree observations required')
    observed, total, inventory = {}, 0, set()
    for name, pin in vendors.items():
        prefix = pin['replacementPath'][2:] + '/'
        entries = {}
        for path, raw in files.items():
            if not path.startswith(prefix):
                continue
            relative = path[len(prefix):]
            require(relative and all(part and part not in ('.', '..') for part in relative.split('/')) and
                    '\\' not in relative and not any(ord(c) < 32 or ord(c) == 127 for c in relative),
                    'vendor path differs')
            mode = modes.get(path)
            require(mode in ('100644', '100755', '120000') and isinstance(raw, bytes), 'vendor mode differs')
            if mode == '120000':
                require(path == 'third_party/tailscale/.stignore' and raw == b'.gitignore', 'unexpected vendor symlink')
            require(len(raw) <= 32 * 1024 * 1024, 'vendor file exceeds byte budget')
            total += len(raw)
            require(total <= 512 * 1024 * 1024, 'vendor inventory exceeds byte budget')
            entries[relative] = (mode, raw)
            inventory.add(path)
        require(len(entries) == pin['replacementFileCount'], 'vendor inventory differs')
        require(trees[name] == pin['replacementTree'] == vendor_tree(entries), 'vendor subtree differs')
        observed[name] = {'path': pin['replacementPath'], 'tree': pin['replacementTree'], 'files': len(entries)}
    require(set(modes) == inventory, 'vendor mode inventory differs')
    return observed


def verify_vendor_sources(source, vendors, git):
    """Read actual tracked roots; Git transport and clean/tag guards stay with callers."""
    source = Path(source).resolve(strict=True)
    files, modes, trees, total = {}, {}, {}, 0
    for name, pin in vendors.items():
        prefix = pin['replacementPath'][2:]
        trees[name] = git(source, 'rev-parse', 'HEAD:' + prefix)
        require(trees[name] == pin['replacementTree'], 'vendor subtree differs')
        rows = git(source, 'ls-tree', '-r', 'HEAD', '--', prefix).splitlines()
        require(len(rows) == pin['replacementFileCount'], 'vendor inventory differs')
        for row in rows:
            info, path = row.split('\t'); mode, kind, blob = info.split()
            require(path.startswith(prefix + '/') and path not in files and
                    all(part and part not in ('.', '..') for part in path.split('/')) and
                    '\\' not in path and not any(ord(c) < 32 or ord(c) == 127 for c in path), 'vendor path differs')
            target = source / path
            require(all(not parent.is_symlink() for parent in target.parents if parent != source and parent.is_relative_to(source)),
                    'vendor source directory redirected')
            if mode == '120000':
                require(path == 'third_party/tailscale/.stignore' and target.is_symlink() and
                        os.readlink(target) == '.gitignore', 'unexpected vendor symlink')
                raw = os.readlink(target).encode()
            else:
                require(mode in ('100644', '100755') and target.is_file() and not target.is_symlink() and
                        bool(target.stat().st_mode & 0o111) == (mode == '100755'), 'vendor mode differs')
                require(target.stat().st_size <= 32 * 1024 * 1024, 'vendor file exceeds byte budget')
                raw = target.read_bytes()
            require(kind == 'blob' and git_object(b'blob', raw).hex() == blob, 'vendor bytes differ')
            total += len(raw)
            require(total <= 512 * 1024 * 1024, 'vendor inventory exceeds byte budget')
            files[path], modes[path] = raw, mode
    return verify_vendor_files(vendors, files, modes, trees), files, modes, trees


def verify_vendor_graph(graph, source, vendors):
    require(isinstance(graph, list) and all(isinstance(row, dict) and isinstance(row.get('Path'), str) for row in graph),
            'actual source graph module rows differ')
    require(len(graph) == len({row['Path'] for row in graph}), 'duplicate module graph row')
    require(all(not row.get('Replace') or isinstance(row['Replace'], dict) for row in graph), 'invalid graph replacement')
    local = {r['Path']: r for r in graph if r.get('Replace') and not r['Replace'].get('Version')}
    require(set(local) == set(vendors), 'actual graph replacements differ')
    root = Path(source)
    require(root.is_absolute() and '..' not in root.parts, 'absolute source graph root required')
    physical_root = root.resolve(strict=True)
    for name, row in local.items():
        replacement, pin = row['Replace'], vendors[name]
        expected = root / pin['replacementPath'][2:]
        directory = replacement.get('Dir')
        require(replacement.get('Path') == pin['replacementPath'] and isinstance(directory, str) and
                directory == str(expected) and expected.is_dir() and
                expected.resolve(strict=True).is_relative_to(physical_root), 'local graph path differs')
        require(not row.get('Main') and row.get('Sum', '') == '' and replacement.get('Sum', '') == '' and
                replacement.get('Version', '') == '', 'local replacement must not invent h1/version')
        if 'version' in pin:
            require(row.get('Version') == pin['version'], 'actual vendor graph version differs: ' + name)
    return graph


def decode_binding(raw):
    def pairs(rows):
        result = {}
        for key, value in rows:
            require(key not in result, 'duplicate binding field')
            result[key] = value
        return result
    require(isinstance(raw, bytes) and 0 < len(raw) <= 2 * 1024 * 1024, 'bounded binding required')
    return json.loads(raw, object_pairs_hook=pairs,
                      parse_constant=lambda _: (_ for _ in ()).throw(RuntimeError('nonfinite source binding')))


def source_module_declarations(raw):
    """Read the reviewed go.mod subset without invoking Go or following local paths.

    These exact-source contracts deliberately reject quoted/ambiguous declarations;
    this is a provenance predicate for reviewed bytes, not a general Go parser.
    """
    require(isinstance(raw, bytes) and 0 < len(raw) <= 2 * 1024 * 1024,
            'bounded source go.mod required')
    text = raw.decode('utf-8')
    require('/*' not in text and '\x00' not in text, 'unsupported source module syntax')
    values, dependencies, replacements, block = {}, {}, {}, None
    for raw_line in text.splitlines():
        line = raw_line.split('//', 1)[0].strip()
        if not line:
            continue
        if line == ')':
            require(block is not None, 'unmatched source module block')
            block = None
            continue
        tokens = line.split()
        directive = block or tokens.pop(0)
        if tokens == ['(']:
            require(block is None and directive in ('require', 'replace', 'exclude', 'retract'),
                    'unsupported source module block')
            block = directive
            continue
        require('"' not in line and '`' not in line, 'quoted source module declaration refused')
        if directive in ('module', 'go', 'toolchain'):
            require(block is None and directive not in values and len(tokens) == 1,
                    'duplicate or malformed source module directive')
            values[directive] = tokens[0]
        elif directive == 'require':
            require(len(tokens) == 2 and tokens[0] not in dependencies,
                    'duplicate or malformed source dependency')
            dependencies[tokens[0]] = tokens[1]
        elif directive == 'replace':
            require(tokens.count('=>') == 1, 'malformed source replacement')
            marker = tokens.index('=>'); left, right = tokens[:marker], tokens[marker + 1:]
            require(len(left) in (1, 2) and len(right) in (1, 2) and left[0] not in replacements,
                    'duplicate or malformed source replacement')
            replacements[left[0]] = (left[1:] or [], right)
    require(block is None, 'unterminated source module block')
    return values, dependencies, replacements


def verify_source_policy_files(policy, context, files, actual_meta_tree, *, vendor_modes=None, vendor_trees=None):
    """V1 retains its Meta proof; v2/v3 require every declared vendor bytes/modes/tree.

    V2 link entries contain the link-target bytes, never followed file contents.
    Archive callers without that complete evidence fail closed rather than
    treating their existing Meta-only inventory as a complete reviewed-vendor source proof.
    """
    validate_source_policy(policy, context)
    meta = policy['metacubex']
    vendors = policy.get('vendors')
    reviewed_vendors = verify_vendor_files(vendors, files, vendor_modes, vendor_trees) if vendors else None
    require(actual_meta_tree == meta['replacementTree'], 'actual Meta subtree differs')
    for name, key in [('go.mod', 'goModSha256'), ('go.sum', 'goSumSha256')]:
        require(name in files and sha(files[name]) == policy[key], 'actual ' + name + ' differs')
    directives, dependencies, replacements = source_module_declarations(files['go.mod'])
    require(directives == {'module': 'github.com/sagernet/sing-box',
                           'go': policy['goDirective'], 'toolchain': policy['toolchain']},
            'actual Go/module directives differ')
    require(dependencies.get(meta['module']) == meta['version'] and
            dependencies.get('github.com/sagernet/gomobile') == 'v0.1.13',
            'actual Meta/generator dependency version differs')
    local = {}
    for name, (version, target) in replacements.items():
        if len(target) == 1:
            # A versionless replacement is always local. Include ../, absolute
            # and platform-specific paths in the inventory so none disappear.
            require(not version, 'version-scoped local replacement refused')
            local[name] = target[0]
    if vendors:
        verify_vendor_module_declarations(files['go.mod'], vendors)
    else:
        require(local == {'github.com/metacubex/utls': './third_party/metacubex-utls',
                          'github.com/sagernet/sing-tun': './third_party/sing-tun'},
                'undeclared local replacement inventory')
    require(meta['bindingPath'] in files and sha(files[meta['bindingPath']]) == meta['bindingSha256'],
            'actual Meta binding differs')
    binding = decode_binding(files[meta['bindingPath']])
    require(isinstance(binding, dict), 'binding object required')
    expected = {'schema': 'polaris-repository-module-replacement-v1', 'module': meta['module'],
                'upstreamRepository': 'https://github.com/MetaCubeX/utls',
                'upstreamCommit': meta['upstreamCommit'], 'upstreamVersion': meta['version'],
                'upstreamH1': meta['upstreamH1'], 'upstreamModuleZipSHA256': meta['upstreamZipSha256'],
                'replacementPath': meta['replacementPath'], 'patch': meta['patchPath'],
                'patchSHA256': meta['patchSha256'], 'changedFiles': ['conn.go', 'reality.go']}
    require(all(binding.get(key) == value for key, value in expected.items()), 'Meta baseline/patch binding differs')
    inventory = binding.get('files')
    prefix = meta['replacementPath'][2:] + '/'
    actual = {name[len(prefix):] for name in files if name.startswith(prefix)}
    require(isinstance(inventory, dict) and len(inventory) == 355 and set(inventory) == actual,
            'complete 355-file Meta inventory required')
    changed = []
    for name, pin in inventory.items():
        require(isinstance(name, str) and all(part and part not in ('.', '..') for part in name.split('/')) and
                not name.startswith('/') and '\\' not in name and '\0' not in name,
                'Meta file path differs')
        require(isinstance(pin, dict) and set(pin) == {'beforeSHA256', 'afterSHA256', 'bytes'} and
                hexadecimal(pin['beforeSHA256'], 64) and hexadecimal(pin['afterSHA256'], 64) and
                type(pin['bytes']) is int and pin['bytes'] >= 0, 'Meta file pin differs')
        raw = files[prefix + name]
        require(len(raw) == pin['bytes'] and sha(raw) == pin['afterSHA256'], 'actual Meta bytes differ: ' + name)
        if pin['beforeSHA256'] != pin['afterSHA256']:
            changed.append(name)
    require(sorted(changed) == ['conn.go', 'reality.go'], 'Meta changed-file inventory differs')
    patch = files.get(meta['patchPath'])
    require(isinstance(patch, bytes) and 0 < len(patch) <= 2 * 1024 * 1024 and
            sha(patch) == meta['patchSha256'], 'actual Meta patch differs')
    return {'schema': 'polaris-reviewed-source-observation-v2' if vendors else 'polaris-reviewed-source-observation-v1',
            'sourcePolicySha256': sha(canonical(policy)),
            'replacementTree': actual_meta_tree, 'replacementFileCount': 355,
            'bindingSha256': meta['bindingSha256'], 'patchSha256': meta['patchSha256'],
            **({'vendors': reviewed_vendors} if vendors else {}),
            'baselineH1Role': 'upstream provenance only; local replacement has no h1'}
