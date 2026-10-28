#!/usr/bin/env python3
"""One C4 product-window draft. All remote work is behind local review gates."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys

HERE = Path(__file__).resolve().parent
FROZEN = HERE.parent / 'pc-mac-masque-udp-v3-2026-09-28'
FROZEN_RUN = FROZEN / 'handshake_run.py'
FROZEN_PREPARE = FROZEN / 'handshake_prepare.py'
WORKER = HERE.parent / 'pc-mesh-physical-2026-09-28/unix-worker.py'
SOURCE_HEAD = 'b609f959f57ce34416c51c7b87ce4a76f2e1df56'
CMD_RUN_SHA = 'f693f3714319acccf37bc5c1917308e69496259610271c508f007397ead4c6a5'
HASHES = {
    FROZEN_RUN: 'b0de5334821385b04f330dada6d1fa70c8a6c9fb5c5aed2b05463976d947753a',
    FROZEN_PREPARE: 'cee909a9f13d7e95fdfbc31eabc278c3af3c2ac35f200ca0faa8c7fade5ade89',
    WORKER: 'abf69f63f62c97a27cba4a631dcdee627a97e7758b21c87bdcd39a807f658cf3',
}
MAC_TAGS = {
    'badlinkname', 'tfogo_checklinkname0', 'with_acme', 'with_ccm',
    'with_clash_api', 'with_cloudflared', 'with_dhcp', 'with_gvisor',
    'with_naive_outbound', 'with_ocm', 'with_openconnect', 'with_openvpn',
    'with_quic', 'with_tailscale', 'with_usbip', 'with_utls', 'with_wireguard',
}
MODE = ('raw', 'dialudp')
GENERATOR_HEAD = '7e3d8e8fe8c3dd5c105a6f462805b5a7093240e6'
GENERATOR_CONTRACT = 'frozen-prepare-isolated-cargo-path-v3'
GENERATOR_COMMON_REPO = Path('/home/sway/Code/polaris')
GENERATOR_LOCK_SHA = '590a33302b04205b90ce1a4ac0c55ae7f0199519c49fb0980b93e7cd213121fa'
GENERATOR_MAIN_DEP = (b'polaris-config-engine = { path = '
                      b'"/home/sway/Code/polaris/crates/config-engine" }')
# No runtime environment values are supplied by this source or CI. The
# reviewed seal and nonce must be explicitly selected for one local window.
RUN_NONCE_ENV = 'POLARIS_C4_RUN_NONCE'
RUN_SEAL_SHA_ENV = 'POLARIS_C4_RUN_SEAL_SHA256'
AUDIT_NONCE_ENV = 'POLARIS_C4_AUDIT_NONCE'
# Reviewed exact Clean still requires one-window nonce, seal and audit opt-in.
EXACT_CLEAN_READY = True
PRECLEAN_TRACE_MAX_BYTES = 268435456
PRECLEAN_FINAL_MAX_BYTES = 1048576


def need(ok, reason):
    if not ok:
        raise ValueError(reason)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def private_root(path, prefix):
    need(path.parent == Path('/var/tmp') and
         re.fullmatch(prefix + r'[0-9a-f]{8}', path.name),
         'unexpected private root path')
    st = path.lstat()
    need(stat.S_ISDIR(st.st_mode) and not path.is_symlink() and
         st.st_uid == os.getuid() and stat.S_IMODE(st.st_mode) == 0o700,
         'private root owner/mode changed')


def generator_repo_gate(repo, fixture):
    """Accept only a clean detached worktree of the frozen r5 source.

    The worktree is supplied locally; this function never creates, fetches or
    mutates it. The main checkout is used only to identify its common Git dir.
    """
    repo = Path(repo)
    need(repo.parent == Path('/var/tmp') and
         re.fullmatch(r'pc-mac-masque-generator-[0-9a-f]{8}', repo.name) and
         repo.resolve() == repo,
         'generator repo must be a private exact /var/tmp worktree')
    private_root(repo, 'pc-mac-masque-generator-')

    def git(where, *args, check=True):
        result = subprocess.run(['git', '-C', str(where), *args],
                                capture_output=True, text=True, timeout=15)
        if check:
            need(result.returncode == 0,
                 'generator worktree Git inspection failed')
        return result

    common = git(repo, 'rev-parse', '--path-format=absolute',
                 '--git-common-dir').stdout.strip()
    main_common = git(GENERATOR_COMMON_REPO, 'rev-parse', '--path-format=absolute',
                      '--git-common-dir').stdout.strip()
    need(common == main_common and
         git(repo, 'rev-parse', '--show-toplevel').stdout.strip() == str(repo),
         'generator worktree is not detached from the reviewed Polaris repository')
    detached = git(repo, 'symbolic-ref', '-q', 'HEAD', check=False)
    need(detached.returncode == 1 and not detached.stdout and
         not detached.stderr,
         'generator worktree HEAD must be detached')
    head = git(repo, 'rev-parse', 'HEAD').stdout.strip()
    config_tree = git(repo, 'rev-parse',
                      'HEAD:crates/config-engine').stdout.strip()
    helper_tree = git(repo, 'rev-parse',
                      'HEAD:crates/helper-proto').stdout.strip()
    need(head == GENERATOR_HEAD and head in fixture.HEADS and
         config_tree == fixture.EXPECTED['config_engine_tree'] and
         helper_tree == fixture.EXPECTED['helper_proto_tree'] and
         not git(repo, 'status', '--porcelain',
                 '--untracked-files=all').stdout.strip(),
         'generator worktree HEAD/tree/cleanliness differs from frozen r5')
    return {
        'generatorRepo': str(repo),
        'generatorHead': head,
        'generatorConfigEngineTree': config_tree,
        'generatorHelperProtoTree': helper_tree,
        'generatorConfigSourceSha256': fixture.EXPECTED['config_source'],
        'physicalPrepareSha256': fixture.EXPECTED['physical_prepare'],
    }


def expected_generator_manifest(repo, fixture):
    source = fixture.PHYSICAL / 'generator/Cargo.toml'
    original = source.read_bytes()
    need(digest(source) == fixture.EXPECTED['physical_manifest'] and
         original.count(GENERATOR_MAIN_DEP) == 1 and
         original.count(b'/home/sway/Code/polaris') == 1,
         'frozen generator manifest dependency is not the single reviewed path')
    replacement = (b'polaris-config-engine = { path = "' +
                   str(Path(repo) / 'crates/config-engine').encode() + b'" }')
    return original.replace(GENERATOR_MAIN_DEP, replacement, 1)


def generator_bundle_gate(root, repo, fixture):
    """Prove the isolated Cargo inputs resolve to the pinned historical tree."""
    root = Path(root)
    repo = Path(repo)
    private_root(root, 'pc-mac-masque-generator-build-')
    generator = root / 'generator'
    source_dir = generator / 'src'
    for directory in (generator, source_dir):
        private_root_child = directory.lstat()
        need(stat.S_ISDIR(private_root_child.st_mode) and
             not directory.is_symlink() and
             private_root_child.st_uid == os.getuid() and
             stat.S_IMODE(private_root_child.st_mode) == 0o700,
             'isolated Cargo directory identity changed')
    need(set(root.iterdir()) == {generator} and
         set(generator.iterdir()) == {source_dir, generator / 'Cargo.toml',
                                     generator / 'Cargo.lock'} and
         set(source_dir.iterdir()) == {source_dir / 'main.rs'},
         'isolated Cargo file set changed')
    manifest = generator / 'Cargo.toml'
    lock = generator / 'Cargo.lock'
    source = source_dir / 'main.rs'
    for path in (manifest, lock, source):
        row = path.lstat()
        need(stat.S_ISREG(row.st_mode) and not path.is_symlink() and
             row.st_uid == os.getuid() and stat.S_IMODE(row.st_mode) == 0o600,
             'isolated Cargo file identity changed')
    frozen = fixture.PHYSICAL / 'generator'
    need(manifest.read_bytes() == expected_generator_manifest(repo, fixture) and
         digest(lock) == GENERATOR_LOCK_SHA == digest(frozen / 'Cargo.lock') and
         digest(source) == fixture.EXPECTED['physical_generator'] ==
         digest(frozen / 'src/main.rs'),
         'isolated Cargo manifest/lock/source differs from pinned inputs')
    result = subprocess.run(
        ['cargo', 'metadata', '--offline', '--locked', '--format-version', '1',
         '--manifest-path', str(manifest)],
        check=True, capture_output=True, text=True, timeout=60)
    metadata = json.loads(result.stdout)
    names = ('polaris-pc-mesh-physical-generator',
             'polaris-config-engine', 'polaris-helper-proto')
    paths = {
        names[0]: str(manifest),
        names[1]: str(repo / 'crates/config-engine/Cargo.toml'),
        names[2]: str(repo / 'crates/helper-proto/Cargo.toml'),
    }
    for value in paths.values():
        package_manifest = Path(value)
        entity = package_manifest.lstat()
        need(stat.S_ISREG(entity.st_mode) and
             not package_manifest.is_symlink() and
             package_manifest.resolve() == package_manifest and
             entity.st_uid == os.getuid(),
             'Cargo metadata local manifest entity changed')
    local = {row['name']: row for row in metadata['packages']
             if row.get('source') is None}
    need(set(local) == set(names) and
         {name: local[name]['manifest_path'] for name in names} == paths and
         metadata.get('workspace_root') == str(generator),
         'Cargo metadata resolved an unreviewed local source')
    nodes = {row['id']: row for row in metadata['resolve']['nodes']}
    ids = {name: local[name]['id'] for name in names}
    need(metadata['resolve']['root'] == ids[names[0]] and
         ids[names[1]] in {d['pkg'] for d in
                           nodes[ids[names[0]]]['deps']} and
         ids[names[2]] in {d['pkg'] for d in
                           nodes[ids[names[1]]]['deps']},
         'Cargo metadata dependency edges differ')
    return {
        'generatorBuildRoot': str(root),
        'generatorCargoManifestSha256': digest(manifest),
        'generatorCargoLockSha256': digest(lock),
        'generatorSourceSha256': digest(source),
        'cargoResolvedManifests': paths,
        'cargoResolvedEdges': [names[0] + '>' + names[1],
                               names[1] + '>' + names[2]],
    }


def generator_depfile_gate(prepared, repo):
    """Read rustc's actual fresh-target source paths for both local crates."""
    prepared = Path(prepared)
    repo = Path(repo)
    private_root(prepared, r'pc-mac-masque-handshake-prepared-')
    deps = prepared / 'cargo-target/debug/deps'
    for directory in (prepared / 'cargo-target',
                      prepared / 'cargo-target/debug', deps):
        entity = directory.lstat()
        need(stat.S_ISDIR(entity.st_mode) and
             not directory.is_symlink() and
             directory.resolve() == directory and
             entity.st_uid == os.getuid() and
             stat.S_IMODE(entity.st_mode) & 0o022 == 0 and
             entity.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX) == 0,
             'compiled generator cargo-target directory identity changed')
    found = {}
    for name, crate, directory in (
            ('polaris-config-engine', 'polaris_config_engine', 'config-engine'),
            ('polaris-helper-proto', 'polaris_helper_proto', 'helper-proto')):
        matches = list(deps.glob(crate + '-*.d'))
        need(len(matches) == 1, 'compiled generator depfile count differs')
        path = matches[0]
        row = path.lstat()
        need(stat.S_ISREG(row.st_mode) and not path.is_symlink() and
             row.st_uid == os.getuid(),
             'compiled generator depfile entity changed')
        content = path.read_text()
        first_target = content.split(':', 1)[0]
        need(first_target == str(path),
             'compiled generator depfile target is stale or external')
        raw_paths = re.findall(r'/[^\s:]+\.rs\b', content)
        sources = {Path(value) for value in raw_paths}
        expected = repo / 'crates' / directory / 'src'
        need(sources and expected / 'lib.rs' in sources and
             all(path.is_file() and not path.is_symlink() and
                 path.resolve() == path and path.is_relative_to(expected)
                 for path in sources),
             'compiled generator depfile source escaped historical worktree')
        found[name] = {
            'path': str(path), 'sha256': digest(path),
            'sources': sorted(str(path) for path in sources),
        }
    return found


def generator_binary_gate(prepared):
    prepared = Path(prepared)
    binary = prepared / 'cargo-target/debug/polaris-pc-mesh-physical-generator'
    row = binary.lstat()
    need(stat.S_ISREG(row.st_mode) and not binary.is_symlink() and
         row.st_uid == os.getuid() and
         digest(binary) ==
         json.loads((prepared / 'manifest.json').read_text())['generator_sha256'],
         'compiled generator binary differs from physical manifest')
    return digest(binary)


def load_prepare():
    """Load the reviewed local source, independent of sys.modules['prepare']."""
    path = HERE / 'prepare.py'
    before = digest(path)
    spec = importlib.util.spec_from_file_location('c4_candidate_prepare_source', path)
    need(spec is not None and spec.loader is not None,
         'candidate prepare module cannot be loaded')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    need(Path(module.__file__).resolve() == path and
         Path(module.__spec__.origin).resolve() == path and
         digest(path) == before,
         'candidate prepare module source changed during load')
    return module


def linker_buildinfo_gate(settings, expected_ldflags):
    need(settings.get('-trimpath') == 'true',
         'candidate buildinfo lacks official trimpath build face')
    # DefaultGODEBUG is computed from module compatibility settings before
    # linking. It is recorded in buildinfo but is not a linker-flags witness
    # or an admission predicate without a same-face CGO1 reference value.
    # These fixed flags are absent from the observed Go 1.26.8 trimpath
    # buildinfo. The exact linker command, including -checklinkname=0, is
    # bound by pinned prepare.py, release/LDFLAGS, receipt and attestation.
    if '-ldflags' in settings:
        try:
            flags = json.loads(settings['-ldflags'])
        except (ValueError, TypeError):
            flags = None
        need(flags == expected_ldflags,
             'candidate buildinfo linker flags differ from fixed release/LDFLAGS')


def buildinfo(path, expected_quic_root, expected_ldflags):
    result = subprocess.run(['go', 'version', '-m', str(path)], capture_output=True,
                            text=True, check=True, timeout=15)
    settings = {}
    deps = {}
    replacements = {}
    last_dep = None
    for line in result.stdout.splitlines():
        parts = line.strip().split('\t')
        if len(parts) == 2 and parts[0] == 'build' and '=' in parts[1]:
            key, value = parts[1].split('=', 1)
            need(key not in settings, 'duplicate buildinfo setting')
            settings[key] = value
        elif len(parts) >= 3 and parts[0] == 'dep':
            need(parts[1] not in deps, 'duplicate buildinfo dependency')
            deps[parts[1]] = parts[2]
            last_dep = parts[1]
        elif len(parts) >= 3 and parts[0] == '=>':
            need(last_dep is not None and last_dep not in replacements,
                 'ambiguous buildinfo replacement')
            replacements[last_dep] = parts[1]
            last_dep = None
        else:
            last_dep = None
    need('go1.26.8' in result.stdout.splitlines()[0],
         'candidate Go compiler differs')
    need(settings.get('GOOS') == 'darwin' and settings.get('GOARCH') == 'arm64' and
         settings.get('CGO_ENABLED') == '1', 'candidate is not official Mac arm64 CGO build face')
    tags = settings.get('-tags', '').split(',')
    need(len(tags) == len(set(tags)) and set(tags) == MAC_TAGS,
         'candidate build tags differ from official Mac build face')
    need(deps.get('github.com/sagernet/quic-go') == 'v0.61.0-sing-box-mod.7' and
         deps.get('github.com/sagernet/sing-quic') ==
         'v0.7.1-0.20260924092235-4f371c86a365',
         'candidate QUIC dependency differs')
    need(replacements.get('github.com/sagernet/quic-go') ==
         str(expected_quic_root),
         'candidate buildinfo does not bind private quic-go replacement')
    linker_buildinfo_gate(settings, expected_ldflags)
    return result.stdout


def candidate_artifact_gate(root, source, manifest, prepare,
                            quic_reference=None):
    """Rebuild every private source artifact from the pinned upstream bytes."""
    quic_reference = quic_reference or prepare.QUIC
    expected = {
        root / 'quic-go/polaris_trace_darwin.go':
            (HERE / 'polaris_trace_darwin.go').read_bytes(),
        root / 'quic-go/polaris_socket_state_darwin_cgo.go':
            (HERE / 'polaris_socket_state_darwin_cgo.go').read_bytes(),
        root / 'quic-go/polaris_socket_state_stub.go':
            (HERE / 'polaris_socket_state_stub.go').read_bytes(),
        root / 'quic-go/sys_conn_oob.go':
            prepare.patch_oob(quic_reference / 'sys_conn_oob.go').encode(),
        root / 'quic-go/sys_conn_msgx_darwin.go':
            prepare.patch_msgx(quic_reference / 'sys_conn_msgx_darwin.go').encode(),
        root / 'overlay_route_conn.go':
            prepare.patch_route(source / 'route/conn.go').encode(),
        root / 'overlay_cmd_sing-box_cmd_run.go':
            prepare.patch_cmd_run(source / 'cmd/sing-box/cmd_run.go').encode(),
    }
    for mode in MODE:
        expected[root / ('client_h3_' + mode + '.go')] = \
            prepare.patch_h3(source / 'transport/http/client_h3.go', mode).encode()
    for path, content in expected.items():
        need(path.is_file() and not path.is_symlink() and
             path.read_bytes() == content,
             'candidate artifact differs from pinned source: ' + str(path))
    need(manifest.get('traceSourceSha256') ==
         hashlib.sha256(expected[root / 'quic-go/polaris_trace_darwin.go']).hexdigest(),
         'candidate trace manifest differs from source')
    need(manifest.get('socketStateSourceSha256') == {
        name: hashlib.sha256(expected[root / 'quic-go' / name]).hexdigest()
        for name in prepare.SOCKET_STATE_SOURCES},
         'candidate socket state manifest differs from source')
    expected_mod = ((source / 'go.mod').read_text() +
                    '\nreplace github.com/sagernet/quic-go => ' +
                    str(root / 'quic-go') + '\n').encode()
    need((root / 'build.mod').is_file() and
         not (root / 'build.mod').is_symlink() and
         (root / 'build.mod').read_bytes() == expected_mod and
         (root / 'build.sum').is_file() and
         not (root / 'build.sum').is_symlink() and
         (root / 'build.sum').read_bytes() == (source / 'go.sum').read_bytes(),
         'candidate build.mod/build.sum differs from pinned source')


def quic_tree_gate(reference, candidate, added_or_patched,
                   expected_reference_digest):
    """No unreviewed Go/module bytes may enter the local replace directory."""
    prepare = load_prepare()
    need(prepare.tree_sha(reference) == expected_reference_digest,
         'private quic-go reference tree pin differs')
    def inventory(root):
        need(root.is_dir() and not root.is_symlink(),
             'private quic-go tree root invalid')
        rows = {}
        for path in root.rglob('*'):
            mode = path.lstat().st_mode
            need(stat.S_ISREG(mode) or stat.S_ISDIR(mode),
                 'private quic-go tree contains symlink/special file')
            rows[str(path.relative_to(root))] = 'file' if stat.S_ISREG(mode) else 'dir'
        return rows

    reference_rows = inventory(reference)
    candidate_rows = inventory(candidate)
    expected_rows = dict(reference_rows)
    for name in added_or_patched:
        need('/' not in name and name not in ('.', '..'),
             'private quic-go override path invalid')
        expected_rows[name] = 'file'
    need(candidate_rows == expected_rows,
         'private quic-go tree file/dir set differs')
    for name, kind in expected_rows.items():
        if kind == 'file':
            expected = (added_or_patched[name] if name in added_or_patched
                        else (reference / name).read_bytes())
            need((candidate / name).read_bytes() == expected,
                 'private quic-go tree bytes differ: ' + name)


def distinct_mode_binary_gate(modes):
    need(isinstance(modes, dict) and set(modes) == set(MODE) and
         all(isinstance(modes[mode], dict) and
             isinstance(modes[mode].get('binarySha256'), str) and
             re.fullmatch(r'[0-9a-f]{64}', modes[mode]['binarySha256'])
             for mode in MODE),
         'both comparison candidate binary hashes required')
    need(modes['raw']['binarySha256'] != modes['dialudp']['binarySha256'],
         'RawDialer and DialUDP binaries are byte-identical')


def candidate_gate(root, mode, *, quic_reference=None,
                   sing_quic_reference=None, require_bundle=True):
    need(mode in MODE, 'unknown candidate mode')
    private_root(root, r'polaris-mac-masque-fdtrace-')
    prepare = load_prepare()
    quic_reference = quic_reference or prepare.QUIC
    sing_quic_reference = sing_quic_reference or prepare.SING_QUIC
    manifest_path = root / 'prepare.json'
    manifest = json.loads(manifest_path.read_text())
    need(manifest.get('sourceHead') == SOURCE_HEAD and
         manifest.get('sourcePins') == prepare.PINS and
         manifest.get('quicPins') == prepare.QUIC_PINS and
         manifest.get('singQuicPin') == prepare.SING_QUIC_PIN,
         'full source pin is absent or differs')
    source = Path(manifest['source'])
    nonce = root.name.removeprefix('polaris-mac-masque-fdtrace-')
    need(source == Path('/var/tmp/polaris-mac-masque-source-' + nonce),
         'candidate source/nonce absolute path differs')
    compiled_source = Path('/private' + str(source))
    need(manifest.get('compiledSourceRoot') == str(compiled_source),
         'candidate compiled source realpath differs from Mac overlay contract')
    prepare.check_source(source, quic_reference, sing_quic_reference)
    ldflags = prepare.link_flags(source)
    need(manifest.get('linkerFlagsSha256') == prepare.PINS['release/LDFLAGS'] and
         manifest.get('linkerFlags') == ldflags,
         'candidate linker flags manifest differs from pinned source')
    need(digest(source / 'cmd/sing-box/cmd_run.go') == CMD_RUN_SHA,
         'graceful signal entry source changed')
    candidate_artifact_gate(root, source, manifest, prepare, quic_reference)
    quic_tree_gate(quic_reference, root / 'quic-go', {
        'polaris_trace_darwin.go':
            (HERE / 'polaris_trace_darwin.go').read_bytes(),
        'polaris_socket_state_darwin_cgo.go':
            (HERE / 'polaris_socket_state_darwin_cgo.go').read_bytes(),
        'polaris_socket_state_stub.go':
            (HERE / 'polaris_socket_state_stub.go').read_bytes(),
        'sys_conn_oob.go':
            prepare.patch_oob(quic_reference / 'sys_conn_oob.go').encode(),
        'sys_conn_msgx_darwin.go':
            prepare.patch_msgx(quic_reference / 'sys_conn_msgx_darwin.go').encode(),
    }, prepare.QUIC_TREE_SHA256)
    need(manifest.get('finalizeSourceSha256') == CMD_RUN_SHA and
         isinstance(manifest.get('finalizeOverlaySha256'), str) and
         re.fullmatch(r'[0-9a-f]{64}', manifest['finalizeOverlaySha256']),
         'process-level finalize source/overlay not pinned')
    common_paths = {
        str(compiled_source / name)
        for name in ('route/conn.go', 'cmd/sing-box/cmd_run.go')}
    common_hashes = manifest.get('commonOverlaySha256', {})
    need(set(common_hashes) == common_paths, 'common diagnostic overlays not pinned')
    need(manifest.get('traceSourceSha256') == digest(HERE / 'polaris_trace_darwin.go'),
         'private trace source changed')
    need(manifest.get('socketStateSourceSha256') == {
        name: digest(HERE / name) for name in prepare.SOCKET_STATE_SOURCES},
         'private socket state sources changed')
    abi = manifest.get('socketStateABI')
    need(isinstance(abi, dict) and
         set(abi) == {'sdkPath', 'sdkVersion', 'socketFdinfoSize', 'macroSize',
                      'flavor', 'cannotSendBit'} and
         isinstance(abi['sdkPath'], str) and Path(abi['sdkPath']).is_absolute() and
         isinstance(abi['sdkVersion'], str) and
         re.fullmatch(r'[0-9]+(?:\.[0-9]+)*', abi['sdkVersion']) is not None and
         type(abi['socketFdinfoSize']) is int and
         0 < abi['socketFdinfoSize'] <= 4096 and
         abi['socketFdinfoSize'] == abi['macroSize'] and
         abi['flavor'] == 3 and abi['cannotSendBit'] == 16,
         'public socket state SDK ABI receipt invalid')
    patched = manifest.get('patchedQuicSha256', {})
    for name in ('sys_conn_oob.go', 'sys_conn_msgx_darwin.go'):
        need(patched.get(name) == digest(root / 'quic-go' / name),
             'private quic-go patch changed: ' + name)
    distinct_mode_binary_gate(manifest.get('modes'))
    need(set(manifest.get('buildTags', '').split(',')) == MAC_TAGS,
         'candidate manifest omitted official Mac tags')
    need(manifest.get('buildProfile') == prepare.BUILD_PROFILE and
         manifest.get('quicReferenceTreeSha256') == prepare.QUIC_TREE_SHA256 and
         manifest.get('singQuicReferenceTreeSha256') ==
         prepare.SING_QUIC_TREE_SHA256,
         'candidate full build/reference profile absent')
    need(manifest.get('finalizeContract') == 'process-graceful-exit-v1',
         'process-level finalize contract missing')
    sources = {}
    for candidate_mode in MODE:
        entry = manifest['modes'][candidate_mode]
        overlay = Path(entry['overlay'])
        need(overlay.parent == root and overlay.is_file() and not overlay.is_symlink(),
             'overlay escaped private root')
        mapping = json.loads(overlay.read_text())['Replace']
        source_name = str(compiled_source / 'transport/http/client_h3.go')
        need(set(mapping) == common_paths | {source_name},
             'candidate overlay file set differs')
        for common_path in common_paths:
            private_overlay = Path(mapping[common_path])
            need(private_overlay.parent == root and not private_overlay.is_symlink() and
                 digest(private_overlay) == common_hashes[common_path],
                 'common overlay changed: ' + common_path)
        cmd_path = str(compiled_source / 'cmd/sing-box/cmd_run.go')
        need(digest(Path(mapping[cmd_path])) ==
             manifest['finalizeOverlaySha256'], 'finalize overlay changed')
        patched_h3 = Path(mapping[source_name])
        need(patched_h3.parent == root and not patched_h3.is_symlink() and
             digest(patched_h3) == entry['sourceSha256'], 'candidate H3 patch changed')
        sources[candidate_mode] = patched_h3.read_text()
    need(sources['raw'].replace(
        'c.dialer.DialContext(ctx, N.NetworkUDP, c.server)',
        'net.DialUDP("udp4", nil, c.server.UDPAddr())').replace(
        'PolarisTraceDial(rawConn, "raw")',
        'PolarisTraceDial(rawConn, "dialudp")') == sources['dialudp'],
        'RawDialer and DialUDP candidates differ outside declared call')
    binary = root / ('sing-box-c4-' + mode + '-darwin-arm64')
    need(binary.is_file() and not binary.is_symlink() and
         digest(binary) == manifest['modes'][mode]['binarySha256'],
         'candidate binary not pinned')
    need(manifest['modes'][mode].get('compileWitness') ==
         prepare.compile_witness(binary, mode),
         'candidate compile witness differs from H3/route/run overlay contract')
    info = buildinfo(binary, root / 'quic-go', ldflags)
    need(hashlib.sha256(info.encode()).hexdigest() ==
         manifest['modes'][mode]['buildinfoSha256'],
         'candidate buildinfo not pinned')
    if require_bundle:
        import ci_bundle
        ci_bundle.verify(root, manifest)
    return manifest, binary, manifest_path


def frozen_module(candidate_sha):
    for path, expected in HASHES.items():
        need(digest(path) == expected, 'frozen recovery component changed: ' + str(path))
    need('handshake_prepare' not in sys.modules,
         'frozen prepare module was preloaded before pin check')
    sys.path.insert(0, str(FROZEN))
    spec = importlib.util.spec_from_file_location('c4_frozen_run', FROZEN_RUN)
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
    finally:
        sys.modules.pop('handshake_prepare', None)
        sys.path.remove(str(FROZEN))
    need(Path(module.fixture.__file__).resolve() == FROZEN_PREPARE.resolve() and
         digest(Path(module.fixture.__file__)) == HASHES[FROZEN_PREPARE],
         'executed frozen prepare module source differs')
    # The frozen script requires this one exact candidate SHA in load and start.
    # No recovery, Stop, clean or network predicate is modified.
    module.fixture.EXPECTED['mac_core'] = candidate_sha
    return module


def fixture_gate(prepared, binary, frozen, mode, manifest_path):
    private_root(prepared, r'pc-mac-masque-handshake-prepared-')
    meta, spec = frozen.load(prepared)
    need(meta['sha256']['macCore'] == digest(binary),
         'prepared fixture does not carry selected Mac candidate')
    need(meta['sha256']['linuxCore'] ==
         '64f6d8613f9c7d42ef9a8e90dd9fca7290f176c5b482714915c04353911559c0',
         'Linux core differs from frozen r5 baseline')
    need(spec['node']['address'] == '192.168.10.185' and
         spec['node']['protocol'] == 'masque-client' and
         spec['node']['port'] == meta['port'] and
         spec['node']['masqueClientSettings'] ==
         {'version': 3, 'disable_version_fallback': True},
         'MASQUE fixture changed')
    client = json.loads((prepared / meta['paths']['client']).read_text())
    need(len(client.get('endpoints', [])) == 1 and
         client['endpoints'][0]['server'] == '192.168.10.185' and
         client['endpoints'][0]['server_port'] == meta['port'] and
         client['endpoints'][0]['version'] == 3 and
         client['endpoints'][0]['disable_version_fallback'] is True,
         'client endpoint not exact pinned MASQUE target')
    endpoint = client['endpoints'][0]
    for key in ('detour', 'bind_interface', 'inet4_bind_address', 'inet6_bind_address',
                'routing_mark', 'network_strategy', 'network_type',
                'fallback_network_type', 'network_fallback_delay', 'udp_bind_port'):
        need(key not in endpoint and key not in client.get('route', {}),
             'DialUDP bypasses active RawDialer policy: ' + key)
    need(client.get('route', {}).get('auto_detect_interface') in (None, False) and
         client.get('route', {}).get('default_interface') in (None, ''),
         'DialUDP bypasses interface selection')
    derivation_path = prepared / 'candidate-derivation.json'
    need(derivation_path.is_file() and not derivation_path.is_symlink() and
         stat.S_IMODE(derivation_path.stat().st_mode) == 0o600,
         'candidate fixture derivation receipt absent')
    derivation = json.loads(derivation_path.read_text())
    provenance = generator_repo_gate(derivation.get('generatorRepo', ''),
                                     frozen.fixture)
    build_root = Path(derivation.get('generatorBuildRoot', ''))
    need(build_root == Path('/var/tmp/pc-mac-masque-generator-build-' +
                            prepared.name.rsplit('-', 1)[-1]),
         'isolated Cargo root does not match prepared fixture')
    cargo_provenance = generator_bundle_gate(
        build_root, Path(derivation['generatorRepo']), frozen.fixture)
    depfiles = generator_depfile_gate(prepared,
                                      Path(derivation['generatorRepo']))
    generator_sha = generator_binary_gate(prepared)
    need(meta.get('sourceHead') == GENERATOR_HEAD and
         meta.get('configEngineTree') ==
         provenance['generatorConfigEngineTree'] and
         meta.get('sourceDigest') ==
         provenance['generatorConfigSourceSha256'],
         'prepared fixture source does not match pinned detached worktree')
    expected_derivation = {
        'nonce': meta['nonce'], 'mode': mode, 'port': meta['port'],
        'sourceHead': SOURCE_HEAD,
        'runnerSha256': digest(Path(__file__)),
        'candidateManifestSha256': digest(manifest_path),
        'candidateBinarySha256': digest(binary),
        'preparedSha256': digest(prepared / 'handshake.json'),
        'frozenPrepareSha256': HASHES[FROZEN_PREPARE],
        'deriveScriptSha256': digest(HERE / 'derive_fixture.py'),
        **provenance,
        **cargo_provenance,
        'generatorDepfiles': depfiles,
        'generatorBinarySha256': generator_sha,
        'generatorContract': GENERATOR_CONTRACT,
    }
    need(derivation == expected_derivation and
         digest(FROZEN_PREPARE) == HASHES[FROZEN_PREPARE],
         'candidate fixture derivation receipt mismatch')
    return meta


def seal_gate(seal_path, prepared, candidate, binary, manifest_path, mode, nonce,
              postcheck_procedure):
    import ci_bundle
    seal_stat = seal_path.lstat()
    need(stat.S_ISREG(seal_stat.st_mode) and not seal_path.is_symlink() and
         seal_stat.st_uid == os.getuid() and
         stat.S_IMODE(seal_stat.st_mode) == 0o600,
         'review seal entity/owner/mode changed')
    need(postcheck_procedure.resolve() == (HERE / 'postcheck.py').resolve(),
         'independent postcheck procedure is not the reviewed private source')
    seal = json.loads(seal_path.read_text())
    build_receipt, archive, ci = ci_bundle.verify(
        candidate, json.loads(manifest_path.read_text()))
    ci_namespace = candidate.name.removeprefix('polaris-mac-masque-fdtrace-')
    need(re.fullmatch(r'[0-9a-f]{8}', ci_namespace) is not None and
         ci_bundle.paths(ci_namespace)[2:] == (archive, build_receipt) and
         ci.get('nonce') == ci_namespace,
         'CI artifact namespace differs from verified candidate')
    expected = {
        'decision': 'approved-for-one-window',
        'nonce': nonce, 'mode': mode, 'sourceHead': SOURCE_HEAD,
        'preparedSha256': digest(prepared / 'handshake.json'),
        'candidateManifestSha256': digest(manifest_path),
        'candidateBinarySha256': digest(binary),
        'runnerSha256': digest(Path(__file__)),
        'classifierSha256': digest(HERE / 'classify.py'),
        'prepareSha256': digest(HERE / 'prepare.py'),
        'deriveScriptSha256': digest(HERE / 'derive_fixture.py'),
        'fixtureDerivationSha256': digest(prepared / 'candidate-derivation.json'),
        'frozenRunSha256': HASHES[FROZEN_RUN],
        'frozenPrepareSha256': HASHES[FROZEN_PREPARE],
        'frozenWorkerSha256': HASHES[WORKER],
        'postcheckProcedureSha256': digest(postcheck_procedure),
        'remoteAuditSha256': digest(HERE / 'remote_audit.py'),
        'stageRecoverSha256': digest(HERE / 'stage_recover.py'),
        'ciBundleToolSha256': digest(HERE / 'ci_bundle.py'),
        'ciNamespace': ci_namespace,
        'ciBuildReceiptPath': str(build_receipt),
        'ciArchivePath': str(archive),
        'ciBuildReceiptSha256': digest(build_receipt),
        'ciArchiveSha256': digest(archive),
        'recoveryContract': 'frozen-stop-restore-clean-plus-independent-postcheck',
        'finalizeContract': 'process-graceful-exit-v1',
    }
    need(seal == expected, 'review seal does not bind exact one-window inputs')


def local_gate(args):
    manifest, binary, manifest_path = candidate_gate(args.candidate, args.mode)
    frozen = frozen_module(digest(binary))
    meta = fixture_gate(args.prepared, binary, frozen, args.mode, manifest_path)
    seal_gate(args.seal, args.prepared, args.candidate, binary, manifest_path,
              args.mode, meta['nonce'], args.postcheck_procedure)
    need(not frozen.receipt_path(meta).exists(),
         'frozen receipt already exists; one-window replay forbidden')
    evidence = Path('/var/tmp/pc-mac-masque-fdtrace-evidence-' + meta['nonce'])
    need(not evidence.exists() and not evidence.is_symlink(),
         'private trace evidence root already exists')
    return frozen, meta, evidence, binary, manifest


def preclean_bundle_gate(meta, mode, receipt_path, trace_path, final_path,
                         trace_pull, final_pull):
    """Prove retrieved evidence is complete; defer diagnostic validity."""
    def preflight_artifact(path, pulled, limit):
        need(isinstance(pulled, dict) and
             type(pulled.get('bytes')) is int and
             pulled['bytes'] <= limit,
             'SCP diagnostic pull size limit exceeded')
        need(pulled['bytes'] > 0,
             'SCP returned absent or empty diagnostic artifact')
        named = path.lstat()
        need(stat.S_ISREG(named.st_mode) and named.st_nlink == 1,
             'SCP diagnostic artifact entity changed')
        need(named.st_size > 0, 'SCP returned absent or empty diagnostic artifact')
        need(named.st_size <= limit, 'SCP diagnostic artifact size limit exceeded')
        need(named.st_size == pulled['bytes'],
             'SCP returned absent, empty or changed diagnostic artifact')
        return named

    def stable_artifact(path, pulled, limit, named, keep_payload=False):
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK |
                     os.O_CLOEXEC)
        try:
            opened = os.fstat(fd)
            identity = lambda row: (row.st_dev, row.st_ino, row.st_mode,
                                    row.st_nlink, row.st_uid, row.st_size,
                                    row.st_mtime_ns, row.st_ctime_ns)
            need(stat.S_ISREG(opened.st_mode) and opened.st_nlink == 1 and
                 identity(named) == identity(opened) and opened.st_size <= limit,
                 'SCP diagnostic artifact entity changed')
            digest_state = hashlib.sha256()
            newline_count = 0
            last_byte = b''
            total = 0
            chunks = [] if keep_payload else None
            while True:
                block = os.read(fd, min(1048576, limit - total + 1))
                if not block:
                    break
                total += len(block)
                need(total <= limit and total <= opened.st_size,
                     'SCP diagnostic artifact grew during read')
                digest_state.update(block)
                newline_count += block.count(b'\n')
                last_byte = block[-1:]
                if keep_payload:
                    chunks.append(block)
            need(total == opened.st_size,
                 'SCP diagnostic artifact changed during read')
            need(identity(opened) == identity(os.fstat(fd)) ==
                 identity(path.lstat()),
                 'SCP diagnostic artifact changed during read')
            digest_hex = digest_state.hexdigest()
            need(pulled['bytes'] == total and pulled.get('sha256') == digest_hex,
                 'SCP returned absent, empty or changed diagnostic artifact')
            return {'sha256': digest_hex, 'newlines': newline_count,
                    'lastByte': last_byte,
                    'payload': b''.join(chunks) if keep_payload else None}
        finally:
            os.close(fd)

    trace_named = preflight_artifact(
        trace_path, trace_pull, PRECLEAN_TRACE_MAX_BYTES)
    final_named = preflight_artifact(
        final_path, final_pull, PRECLEAN_FINAL_MAX_BYTES)
    trace = stable_artifact(
        trace_path, trace_pull, PRECLEAN_TRACE_MAX_BYTES, trace_named)
    final_artifact = stable_artifact(
        final_path, final_pull, PRECLEAN_FINAL_MAX_BYTES, final_named,
        keep_payload=True)
    receipt = json.loads(receipt_path.read_text())
    need(receipt.get('phase') == 'stopped' and receipt.get('errors') == [] and
         receipt.get('nonce') == meta['nonce'] and receipt.get('port') == meta['port'],
         'frozen Stop receipt incomplete before Clean')
    need(receipt.get('diagnosis', {}).get('captureParsedAll') is True and
         receipt['diagnosis'].get('captureComplete') is True and
         receipt['diagnosis'].get('captureCountLimitHit') is False and
         all(receipt.get('stopped', {}).get(h, {}).get('stopped') is True and
             receipt.get('restoration', {}).get(h, {}).get('network', {}).get('equal') is True and
             receipt['restoration'][h]['network'].get('unknownRequired') == [] and
             receipt['restoration'][h].get('protectedEqual') is True and
             receipt['restoration'][h].get('processesEqual') is True and
             receipt['restoration'][h].get('originalAppEqual') is True
             for h in ('linux', 'mac')),
         'frozen Stop/restoration/capture incomplete before Clean')
    need(trace['lastByte'] == b'\n' and trace['newlines'] > 0,
         'retrieved trace is not complete newline records')
    raw_final = final_artifact['payload']
    need(final_artifact['lastByte'] == b'\n' and
         final_artifact['newlines'] == 1,
         'process final receipt missing complete line')
    final = json.loads(raw_final)
    need(isinstance(final, dict), 'process final receipt invalid')
    pid = receipt.get('clientStart', {}).get('core', {}).get('pid')
    need(type(pid) is int and pid > 0 and
         final.get('nonce') == meta['nonce'] and final.get('pid') == pid and
         final.get('traceSha256') == trace['sha256'] and
         final.get('finalized') is True and final.get('exit') == 'graceful' and
         all(type(final.get(k)) is int for k in
             ('emitted', 'enqueued', 'written', 'dropped', 'writeErrors',
              'identityGaps', 'lateEvents')) and
         final['emitted'] == final['enqueued'] == final['written'] ==
         trace['newlines'] and
         final['dropped'] == final['writeErrors'] ==
         final['identityGaps'] == final['lateEvents'] == 0,
         'process final receipt nonce/PID/hash/count/finalize mismatch')


def execute(args):
    # Direct import callers must pass the same local source, fixture, review
    # seal and one-window absence gates as the CLI before any mkdir or SSH.
    need(re.fullmatch(r'[0-9a-f]{8}',
                      os.environ.get(RUN_NONCE_ENV, '')) is not None and
         re.fullmatch(r'[0-9a-f]{64}',
                      os.environ.get(RUN_SEAL_SHA_ENV, '')) is not None and
         re.fullmatch(r'[0-9a-f]{8}',
                      os.environ.get(AUDIT_NONCE_ENV, '')) is not None,
         'execute disabled: explicit nonce/review seal/audit runtime opt-in absent')
    frozen, meta, evidence, binary, manifest = local_gate(args)
    need(os.environ.get(RUN_NONCE_ENV) == meta['nonce'] and
         os.environ.get(RUN_SEAL_SHA_ENV) == digest(args.seal) and
         os.environ.get(AUDIT_NONCE_ENV) == meta['nonce'],
         'execute disabled: nonce/review seal/audit runtime opt-in absent or mismatched')
    need(EXACT_CLEAN_READY,
         'execute disabled: exact post-Stop Clean review gate closed')
    old_umask = os.umask(0o077)
    try:
        evidence.mkdir(mode=0o700)
    finally:
        os.umask(old_umask)
    record = {'nonce': meta['nonce'], 'mode': args.mode,
              'candidateSha256': digest(binary), 'sourceHead': SOURCE_HEAD,
              'candidateManifestPath': str(args.candidate / 'prepare.json'),
              'candidateManifestSha256': digest(args.candidate / 'prepare.json'),
              'preparedPath': str(args.prepared),
              'preparedSha256': digest(args.prepared / 'handshake.json'),
              'fixtureDerivationSha256': digest(
                  args.prepared / 'candidate-derivation.json'),
              'sealPath': str(args.seal),
              'sealSha256': digest(args.seal),
              'phase': 'local-gates-passed'}
    frozen.save_json(evidence / 'runner.json', record)
    # stage includes the frozen remote preflight. It is reached only after all
    # local gates, including the review seal, pass.
    frozen.stage(args.prepared)
    frozen.run(args.prepared)  # its finally owns exact Stop and restoration
    record['phase'] = 'stopped-restored'
    root = meta['remoteRoots']['mac']
    for remote_name, local_name in (
            ('masque-fdtrace.ndjson', 'masque-fdtrace.ndjson'),
            ('masque-fdtrace.final.json', 'masque-fdtrace.final.json')):
        record[local_name] = frozen.pull_log(
            'mac', root + '/' + remote_name, evidence / local_name)
    preclean_bundle_gate(
        meta, args.mode, frozen.receipt_path(meta),
        evidence / 'masque-fdtrace.ndjson',
        evidence / 'masque-fdtrace.final.json',
        record['masque-fdtrace.ndjson'],
        record['masque-fdtrace.final.json'])
    # A failed transfer or content check preserves both nonce roots.
    import stage_recover
    frozen.EXACT_CLEAN = lambda prepared, meta, data: \
        stage_recover.clean_stopped(
            prepared, meta, data, frozen,
            capability=stage_recover.CLEAN_CAPABILITY)
    try:
        frozen.clean(args.prepared)
    finally:
        frozen.EXACT_CLEAN = None
    record['phase'] = 'clean-awaiting-independent-postcheck'
    record['receiptSha256'] = digest(frozen.receipt_path(meta))
    frozen.save_json(evidence / 'runner.json', record)
    return record


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('action', choices=('plan', 'execute'))
    parser.add_argument('--prepared', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--mode', choices=MODE, required=True)
    parser.add_argument('--seal', type=Path, required=True)
    parser.add_argument('--postcheck-procedure', type=Path, required=True)
    args = parser.parse_args()
    if args.action == 'execute' and (not re.fullmatch(
            r'[0-9a-f]{8}', os.environ.get(RUN_NONCE_ENV, '')) or
            not re.fullmatch(r'[0-9a-f]{64}',
                             os.environ.get(RUN_SEAL_SHA_ENV, '')) or
            not re.fullmatch(r'[0-9a-f]{8}',
                             os.environ.get(AUDIT_NONCE_ENV, ''))):
        print(json.dumps({'ready': False, 'remoteTouched': False,
                          'reason': 'execute disabled: explicit nonce/review seal/audit '
                          'runtime opt-in absent'}, indent=2))
        raise SystemExit(2)
    try:
        frozen, meta, evidence, binary, manifest = local_gate(args)
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(json.dumps({'ready': False, 'remoteTouched': False,
                          'reason': type(error).__name__ + ': ' + str(error)}, indent=2))
        raise SystemExit(2)
    if args.action == 'plan':
        print(json.dumps({'ready': EXACT_CLEAN_READY, 'remoteTouched': False,
                          'executeBlock': None if EXACT_CLEAN_READY else
                          'exact post-Stop Clean review gate closed',
                          'nonce': meta['nonce'], 'mode': args.mode,
                          'sourceHead': SOURCE_HEAD, 'binarySha256': digest(binary),
                          'recovery': 'frozen Stop then restore then Clean; independent postcheck required',
                          'dialudpComparable': args.mode == 'raw' or
                          'fixture policy gate passed; direct socket path remains diagnostic'},
                         indent=2))
        return
    if os.environ.get(AUDIT_NONCE_ENV) != meta['nonce']:
        print(json.dumps({'ready': False, 'remoteTouched': False,
                          'reason': 'execute disabled: audit nonce runtime opt-in '
                          'mismatched'}, indent=2))
        raise SystemExit(2)
    print(json.dumps(execute(args), indent=2))


if __name__ == '__main__':
    main()
