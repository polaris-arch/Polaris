"""Exact reviewed source facts; no candidate selection, build or publication."""
import hashlib
import json
import re


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
    require(isinstance(policy, dict) and set(policy) == fields and
            policy['schema'] == 'polaris-reviewed-kernel-source-v1', 'complete explicit declaration required')
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
    return policy


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


def verify_source_policy_files(policy, context, files, actual_meta_tree):
    """Callers supply regular Git/source-tar files and an independently computed subtree."""
    validate_source_policy(policy, context)
    meta = policy['metacubex']
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
    return {'schema': 'polaris-reviewed-source-observation-v1', 'sourcePolicySha256': sha(canonical(policy)),
            'replacementTree': actual_meta_tree, 'replacementFileCount': 355,
            'bindingSha256': meta['bindingSha256'], 'patchSha256': meta['patchSha256'],
            'baselineH1Role': 'upstream provenance only; local replacement has no h1'}
