#!/usr/bin/env python3
"""Same-run installed tooling identity; never invoke a driver to ask its version."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess

VERSION = '2.0.5'
PACKAGE = 'tauri-driver 2.0.5 (registry+https://github.com/rust-lang/crates.io-index)'
NATIVE = Path('/usr/bin/WebKitWebDriver')
SCHEMA = 'polaris-package-automation-tools-v1'


def require(ok, message):
    if not ok:
        raise ValueError(message)


def regular(path, executable=False):
    path = Path(path).absolute()
    require(not path.is_symlink() and path.is_file()
            and all(not p.is_symlink() for p in path.parents), 'linked or nonregular tool input')
    mode = path.stat().st_mode
    require(not mode & (stat.S_IWGRP | stat.S_IWOTH), 'writable tool input')
    if executable:
        with path.open('rb') as stream:
            require(mode & stat.S_IXUSR and stream.read(4) == b'\x7fELF', 'executable ELF required')
    return path


def identity(path, executable=False):
    path = regular(path, executable)
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest.hexdigest()}


def context(env):
    require(env.get('GITHUB_ACTIONS') == 'true' and env.get('RUNNER_OS') == 'Linux'
            and env.get('GITHUB_REPOSITORY') == 'polaris-arch/Polaris', 'actual Linux Actions tooling context required')
    require(re.fullmatch(r'[0-9a-f]{40}', env.get('GITHUB_SHA', ''))
            and all(re.fullmatch(r'[1-9][0-9]*', env.get(k, '')) for k in ('GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT')),
            'exact tooling source/run/attempt required')
    require(all(env.get(k) for k in ('GITHUB_JOB', 'RUNNER_NAME', 'GITHUB_WORKFLOW_REF', 'RUNNER_TEMP')),
            'same-job runner identity required')
    return {k: env[k] for k in ('GITHUB_REPOSITORY', 'GITHUB_SHA', 'GITHUB_RUN_ID',
            'GITHUB_RUN_ATTEMPT', 'GITHUB_JOB', 'RUNNER_NAME', 'GITHUB_WORKFLOW_REF')}


def fixed_root(env):
    context(env)
    temp = Path(env['RUNNER_TEMP'])
    require(temp.is_absolute() and temp.is_dir() and not temp.is_symlink()
            and all(not p.is_symlink() for p in temp.parents), 'actual unlinked runner temp required')
    return temp / 'polaris-package-automation'


def cargo_package(raw):
    # Cargo's generated single-package v1 install inventory is a bounded subset
    # of TOML. Keep Python 3.10 compatibility, without a third-party parser or
    # accepting an arbitrary package/source merely because it names the binary.
    require(len(raw) <= 16384, 'Cargo inventory bound')
    text = raw.decode('utf-8')
    match = re.fullmatch(r'\s*\[v1\]\s*"([^"\n]+)"\s*=\s*\[\s*"tauri-driver"\s*,?\s*\]\s*', text)
    require(match is not None and match.group(1) == PACKAGE, 'exact Cargo registry package/binary required')
    return PACKAGE


def query(argv):
    return subprocess.check_output(argv, text=True, timeout=15, env={'PATH': '/usr/bin:/bin', 'LC_ALL': 'C'})


def observe(root, env, run=query):
    root = Path(root).absolute()
    require(root == fixed_root(env) and root.is_dir() and not root.is_symlink(), 'fixed private installation root required')
    inventory = regular(root / '.crates.toml')
    require(inventory.stat().st_uid == os.getuid() and inventory.stat().st_size <= 16384,
            'Cargo inventory owner or size differs')
    package = cargo_package(inventory.read_bytes())
    driver = regular(root / 'bin/tauri-driver', executable=True)
    require(driver.stat().st_uid == os.getuid(), 'Cargo executable owner differs')
    native = regular(NATIVE, executable=True)
    require(native.stat().st_uid == 0, 'system WebKit executable owner differs')
    owner = run(['/usr/bin/dpkg-query', '-S', str(native)]).strip()
    require(owner in (f'webkit2gtk-driver: {native}', f'webkit2gtk-driver:amd64: {native}'),
            'WebKit executable package ownership differs')
    fields = run(['/usr/bin/dpkg-query', '-W', '-f=${binary:Package}\t${Version}\t${Architecture}\t${db:Status-Status}\n',
                  'webkit2gtk-driver']).strip().split('\t')
    require(len(fields) == 4 and fields[0] in ('webkit2gtk-driver', 'webkit2gtk-driver:amd64')
            and re.fullmatch(r'[0-9][A-Za-z0-9.+:~\-]*', fields[1])
            and fields[2:] == ['amd64', 'installed'], 'installed WebKit package version/architecture differs')
    return {'schema': SCHEMA, 'context': context(env),
            'automation': {'name': 'tauri-driver', 'version': VERSION, 'package': package,
                           'inventory': identity(inventory), 'binary': identity(driver, True)},
            'nativeDriver': {'package': fields[0], 'version': fields[1], 'architecture': fields[2],
                             'binary': identity(native, True)}}


def prepare(root, output, env=None, run=query):
    env = os.environ if env is None else env
    root = Path(root).absolute(); output = Path(output).absolute()
    require(output == fixed_root(env) / 'receipt.json', 'fixed private receipt path required')
    facts = observe(root, env, run)
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w', encoding='utf-8') as stream:
        json.dump(facts, stream, indent=2); stream.write('\n')
    return facts


def verify(env=None, run=query):
    env = os.environ if env is None else env
    root = fixed_root(env); receipt = regular(root / 'receipt.json')
    require(receipt.stat().st_uid == os.getuid() and stat.S_IMODE(receipt.stat().st_mode) == 0o600
            and receipt.stat().st_size <= 16384, 'private same-user tool receipt required')
    saved = json.loads(receipt.read_text())
    require(saved == observe(root, env, run), 'same-run installed tool identity changed')
    return saved


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare'])
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    prepare(args.root, args.output)
