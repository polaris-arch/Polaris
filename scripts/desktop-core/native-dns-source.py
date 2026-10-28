#!/usr/bin/env python3
"""Offline source proof for one inactive native-DNS staging input.

Reads a previously authorized local Actions index ZIP. No network, executable
payload, source execution, resource installation or release/mobile selection.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import stat
import tempfile
import zipfile

HERE = Path(__file__).resolve().parent
BINDING = HERE / 'native-dns-source-binding.json'
# Syntax parity does not activate this staging-only entry or widen its channels.
POLARIS_VERSION = re.compile(json.loads((HERE.parent / 'lib/polaris-version.json').read_bytes())['pattern'])
spec = importlib.util.spec_from_file_location('native_dns_source_archive', HERE / 'release-archive.py')
archive = importlib.util.module_from_spec(spec)
spec.loader.exec_module(archive)
require = archive.require


def decode(raw, maximum):
    require(0 < len(raw) <= maximum, 'JSON byte limit')

    def pairs(rows):
        result = {}
        for key, value in rows:
            require(key not in result, 'duplicate JSON key')
            result[key] = value
        return result

    def invalid(_):
        raise ValueError('nonfinite JSON value')

    return json.loads(raw, object_pairs_hook=pairs, parse_constant=invalid)


def hexadecimal(value, width):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{%d}' % width, value) and value != '0' * width


def validate(binding):
    fields = {'schema', 'state', 'active', 'AppAccepted', 'publicationEligible', 'repository',
              'repositoryId', 'version', 'tagObject', 'sourceCommit', 'sourceTree', 'producer',
              'indexArtifact', 'index', 'sourceArchive', 'sourceGitlinks', 'sourceIdentity', 'nativeRows'}
    require(isinstance(binding, dict) and set(binding) == fields, 'exact source binding fields required')
    require(binding['schema'] == 'polaris-native-dns-source-binding-v1' and
            binding['state'] == 'inactive-source-verification-only' and
            all(binding[key] is False for key in ('active', 'AppAccepted', 'publicationEligible')) and
            binding['tagObject'] is None, 'staging source proof cannot activate or accept a release')
    require(binding['repository'] == 'polaris-arch/sing-box-polaris' and
            type(binding['repositoryId']) is int and binding['repositoryId'] == 1421657509,
            'source repository differs')
    version = POLARIS_VERSION.fullmatch(binding['version']) if isinstance(binding['version'], str) else None
    require(all(hexadecimal(binding[key], 40) for key in ('sourceCommit', 'sourceTree')) and
            version and version[3] == 'alpha', 'source identity invalid')
    producer = binding['producer']
    require(isinstance(producer, dict) and set(producer) == {'workflow', 'workflowCommit', 'runId', 'runAttempt'} and
            producer['workflow'] == '.github/workflows/build.yml' and
            producer['workflowCommit'] == binding['sourceCommit'] and
            all(type(producer[k]) is int and producer[k] > 0 for k in ('runId', 'runAttempt')),
            'exact staging producer identity required')
    for key, limit in [('indexArtifact', 64 * 1024 ** 2), ('index', 16 * 1024 ** 2),
                       ('sourceArchive', 64 * 1024 ** 2)]:
        value = binding[key]
        fields = {'name', 'bytes', 'sha256'} | ({'id'} if key == 'indexArtifact' else set())
        require(isinstance(value, dict) and set(value) == fields and
                isinstance(value['name'], str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.-]*', value['name']) and
                type(value['bytes']) is int and 0 < value['bytes'] <= limit and hexadecimal(value['sha256'], 64),
                'bounded exact artifact bytes required: ' + key)
    require(type(binding['indexArtifact']['id']) is int and binding['indexArtifact']['id'] > 0 and
            binding['indexArtifact']['name'] == 'binary-native-index' and
            binding['index']['name'] == 'polaris-upstream-native-assets.json' and
            binding['sourceArchive']['name'] == 'sing-box-polaris-source-' + binding['sourceCommit'] + '.tar.gz',
            'source/index artifact identity differs')
    identity = binding['sourceIdentity']
    require(isinstance(identity, dict) and set(identity) == {'sourceOverlay', 'goModSha256', 'goSumSha256', 'replacements'} and
            type(identity['sourceOverlay']) is int and identity['sourceOverlay'] == 0 and
            all(hexadecimal(identity[k], 64) for k in ('goModSha256', 'goSumSha256')) and
            isinstance(identity['replacements'], list), 'zero-overlay source identity required')
    modules = set()
    for row in identity['replacements']:
        require(isinstance(row, dict) and set(row) == {'module', 'path', 'tree'} and
                isinstance(row['module'], str) and row['module'] not in modules and
                isinstance(row['path'], str) and re.fullmatch(r'\./third_party/[a-z0-9-]+', row['path']) and
                hexadecimal(row['tree'], 40), 'source-owned replacement identity invalid')
        modules.add(row['module'])
    require(isinstance(binding['sourceGitlinks'], list) and len(binding['sourceGitlinks']) <= 1024,
            'source gitlink list required')
    names = set()
    for row in binding['sourceGitlinks']:
        require(isinstance(row, dict) and set(row) == {'path', 'sha'} and
                isinstance(row['path'], str) and re.fullmatch(r'clients/[a-z0-9-]+', row['path']) and
                row['path'] not in names and hexadecimal(row['sha'], 40), 'source gitlink identity invalid')
        names.add(row['path'])
    require(type(binding['nativeRows']) is int and binding['nativeRows'] == 44, 'complete 44-row index required')
    return binding


def checked_bytes(raw, pin):
    require(len(raw) == pin['bytes'] and hashlib.sha256(raw).hexdigest() == pin['sha256'],
            'pinned artifact bytes differ')
    return raw


def verify_index(index, binding):
    require(isinstance(index, dict) and index.get('schema') == 'polaris-upstream-native-staging-v1' and
            index.get('tagObject') is None, 'actual staging index required')
    for key in ('repository', 'repositoryId', 'version', 'sourceCommit', 'sourceTree', 'sourceArchive'):
        require(index.get(key) == binding[key], 'index source binding differs: ' + key)
    require(index.get('tag') == 'v' + binding['version'], 'staging version label differs')
    producer = index.get('producer', {})
    require(isinstance(producer, dict) and producer.get('repository') == binding['repository'] and
            producer.get('repositoryId') == binding['repositoryId'] and
            all(producer.get(k) == v for k, v in binding['producer'].items()), 'index producer binding differs')
    rows = index.get('platforms')
    require(isinstance(rows, list) and len(rows) == binding['nativeRows'], 'complete source row coverage required')
    names, replacements = set(), None
    for row in rows:
        require(isinstance(row, dict) and isinstance(row.get('id'), str) and row['id'] and row['id'] not in names,
                'duplicate/missing native row')
        names.add(row['id'])
        require(all(row.get(k) == binding[k] for k in ('sourceCommit', 'sourceTree', 'version', 'sourceIdentity')),
                'native row source binding differs')
        graph = row.get('buildGraph', {})
        settings = graph.get('settings', {})
        require(settings.get('vcs.revision') == binding['sourceCommit'] and settings.get('vcs.modified') == 'false',
                'native index source is modified or differs')
        linked = graph.get('localReplacements')
        require(isinstance(linked, list), 'index replacement declarations missing')
        if replacements is None:
            replacements = linked
        require(linked == replacements, 'native row replacement declarations differ')
    return replacements


def verify(index_zip, binding):
    binding = validate(binding)
    path = Path(index_zip)
    require(path.is_file() and path.stat().st_size == binding['indexArtifact']['bytes'], 'index ZIP size differs')
    checked_bytes(path.read_bytes(), binding['indexArtifact'])
    pins = {binding[key]['name']: binding[key] for key in ('index', 'sourceArchive')}
    with zipfile.ZipFile(path) as package:
        infos = package.infolist()
        require(len(infos) == 2 and {row.filename for row in infos} == set(pins), 'exact two-file index ZIP required')
        contents = {}
        for info in infos:
            require(info.orig_filename == info.filename and not info.flag_bits & 1 and
                    not info.is_dir() and stat.S_IFMT(info.external_attr >> 16) in (0, stat.S_IFREG) and
                    info.file_size == pins[info.filename]['bytes'], 'index ZIP member differs')
            contents[info.filename] = checked_bytes(package.read(info), pins[info.filename])
    index = decode(contents[binding['index']['name']], 16 * 1024 ** 2)
    replacements = verify_index(index, binding)
    # The committed shared verifier reconstructs the whole Git tree, checks
    # module bytes and local subtrees, then compares linked replacement
    # declarations with go.mod/go.sum. Index declarations are not binary proof.
    with tempfile.TemporaryDirectory(prefix='polaris-native-dns-source-') as directory:
        source = Path(directory) / binding['sourceArchive']['name']
        source.write_bytes(contents[source.name])
        tree = archive.verify_source_archive(source, binding['sourceArchive'], binding['sourceCommit'],
                                             binding['sourceTree'], binding['sourceGitlinks'],
                                             binding['sourceIdentity'], replacements)
    return {'scope': 'offline-source-and-index-only', 'sourceCommit': binding['sourceCommit'],
            'sourceTree': tree, 'producerRunId': binding['producer']['runId'],
            'producerRunAttempt': binding['producer']['runAttempt'], 'indexSha256': binding['index']['sha256'],
            'sourceArchiveSha256': binding['sourceArchive']['sha256'], 'nativeIndexRows': len(index['platforms']),
            'sourceVerified': True, 'binaryBytesVerified': False, 'runtimeValidated': False,
            'mobileArtifactsVerified': False, 'AppAccepted': False, 'publicationEligible': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--index-zip', required=True, type=Path, help='already authorized local Actions ZIP')
    args = parser.parse_args()
    print(json.dumps(verify(args.index_zip, decode(BINDING.read_bytes(), 65536)), indent=2))
