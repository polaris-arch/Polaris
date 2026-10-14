#!/usr/bin/env python3
"""Hosted formal Android entry. Explicit isolated tools; no release/publisher action."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess

import sys
sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'

from fork_input import ROOT, canonical, load, require, sha, verify_source
from producer import run, private_dir, producer_code, android_build_code
from budget import native_capacity, native_environment


def prepare_source(source, value):
    source = Path(source)
    if not source.exists():
        run(['git', 'init', '--quiet', source])
        ref = 'refs/tags/' + value['sourceTag']
        run(['git', '-C', source, 'fetch', '--depth=1', '--no-tags', value['sourceURL'], ref + ':' + ref])
        run(['git', '-C', source, 'checkout', '--detach', value['sourceCommit']])
    return verify_source(source, value)


def emit_outputs(paths, fingerprint, root):
    require(re.fullmatch('[0-9a-f]{64}', fingerprint) is not None, 'actual source/tool cache fingerprint required')
    require(all('\n' not in str(p) and '\r' not in str(p) for p in [root, *paths.values()]), 'output paths contain line controls')
    with Path(os.environ['GITHUB_OUTPUT']).open('a') as stream:
        stream.write('fingerprint=' + fingerprint + '\n')
        stream.write('work=' + paths['work'] + '\ncache=' + paths['cache'] + '\n')
    with Path(os.environ['GITHUB_ENV']).open('a') as stream:
        stream.write('POLARIS_FORMAL_MOBILE_ROOT=' + str(root) + '\n')


def location():
    require(os.environ.get('GITHUB_ACTIONS') == 'true', 'hosted Android entry only; local builds use reserved explicit slots')
    root = private_dir(Path.home() / 'Code/polaris-mobile-ci')
    return root


def prepare():
    value = load()  # Fail before tool, repository or cache mutation.
    producer_identity = producer_code()
    root = location()
    tools = private_dir(root / 'tools')
    go = run(['which', 'go']).strip()
    require(run([go, 'version']).startswith('go version go' + value['goVersion'] + ' '), 'actual Go differs')
    tool_cache = private_dir(root / 'tool-cache')
    env = dict(os.environ, GOBIN=str(tools), GOTOOLCHAIN='local', GOWORK='off', GOFLAGS='-mod=readonly',
               GOPATH=str(tool_cache / 'go-path'), GOCACHE=str(tool_cache / 'go-build'),
               GOMODCACHE=str(tool_cache / 'go-mod'))
    observation = native_capacity()
    print(json.dumps({'phase': 'android-tool-prepare-capacity-sample', 'capacity': observation}, sort_keys=True), flush=True)
    env, capacity = native_environment(env, observation)
    print(json.dumps({'phase': 'android-tool-prepare-capacity', 'capacity': capacity}), flush=True)
    run([go, 'install', 'github.com/sagernet/gomobile/cmd/gomobile@' + value['gomobileVersion'],
         'github.com/sagernet/gomobile/cmd/gobind@' + value['gobindVersion']], env=env)
    ndk = run([sys.executable, ROOT / 'scripts/libbox-patches/prepare-ndk-linux.py', root]).strip()
    require(Path(ndk).is_dir(), 'actual NDK preparation failed')
    from android_sdk import observe_ndk_input
    observe_ndk_input(root, ndk, value['android'])
    jdk = Path(os.environ['JAVA_HOME_17_X64']).resolve(strict=True)
    sdk = Path(os.environ.get('ANDROID_SDK_ROOT', os.environ.get('ANDROID_HOME', ''))).resolve(strict=True)
    paths = {'go': go, 'gomobile': str(tools / 'gomobile'), 'gobind': str(tools / 'gobind'),
             'ndk': ndk, 'jdk': str(jdk), 'sdk': str(sdk), 'work': str(private_dir(root / 'android-work')),
             'cache': str(private_dir(root / 'android-cache')), 'source': str(root / 'fork-source')}
    prepare_source(paths['source'], value)
    from artifacts import file_pin
    require(producer_code() == producer_identity, 'App producer source changed during preparation')
    fingerprint = sha(canonical({'source': value, 'code': android_build_code(producer_identity),
                'tools': {name: file_pin(paths[name]) for name in ('go', 'gomobile', 'gobind')},
                'java': {**file_pin((jdk / 'bin/java').resolve(strict=True)), 'version': run([jdk / 'bin/java', '--version'])},
                'javac': {**file_pin((jdk / 'bin/javac').resolve(strict=True)), 'version': run([jdk / 'bin/javac', '--version'])},
                'ndkProperties': (Path(ndk) / 'source.properties').read_text(),
                'bootclasspath': file_pin(sdk / 'platforms/android-36/android.jar')}))
    (root / 'tools.json').write_text(json.dumps(paths, indent=2) + '\n')
    emit_outputs(paths, fingerprint, root)
    print('Formal Android source/tool slots prepared; no bind or publish')


def build():
    root = Path(os.environ['POLARIS_FORMAL_MOBILE_ROOT'])
    paths = json.loads((root / 'tools.json').read_bytes())
    args = [sys.executable, ROOT / 'scripts/mobile-core/producer.py', 'android']
    for name in ('source', 'go', 'gomobile', 'gobind', 'work', 'cache', 'sdk', 'ndk', 'jdk'):
        args += ['--' + name, paths[name]]
    subprocess.run([str(arg) for arg in args], check=True)
    value = load()
    readelf = Path(paths['ndk']) / 'toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf'
    component = Path(paths['work']) / value['android']['outputName']
    receipt = Path(paths['work']) / 'component-receipt.json'
    args = [sys.executable, ROOT / 'scripts/mobile-core/consume.py', 'materialize-android',
            '--component', component, '--receipt', receipt, '--work', private_dir(root / 'consume-work'),
            '--cache', paths['cache'], '--go', paths['go'], '--javap', Path(paths['jdk']) / 'bin/javap', '--readelf', readelf]
    subprocess.run([str(arg) for arg in args], check=True)


def verify(apk=None, abi='arm64-v8a', r8=None):
    root = Path(os.environ['POLARIS_FORMAL_MOBILE_ROOT'])
    paths = json.loads((root / 'tools.json').read_bytes())
    value = load()
    readelf = Path(paths['ndk']) / 'toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf'
    args = [sys.executable, ROOT / 'scripts/mobile-core/consume.py', 'verify-android',
            '--component', Path(paths['work']) / value['android']['outputName'],
            '--receipt', Path(paths['work']) / 'component-receipt.json',
            '--work', private_dir(root / 'verify-work'), '--cache', paths['cache'],
            '--go', paths['go'], '--javap', Path(paths['jdk']) / 'bin/javap', '--readelf', readelf]
    if apk is not None:
        require(r8 is not None, 'actual APK R8 output required')
        args = [sys.executable, ROOT / 'scripts/mobile-core/carriers.py', 'android',
                '--artifact', apk, '--abi', abi, '--r8', r8,
                '--component', Path(paths['work']) / value['android']['outputName'],
                '--receipt', Path(paths['work']) / 'component-receipt.json',
                '--scratch', private_dir(root / 'final-carrier-work'),
                '--output', str(apk) + '.source-consumption.json',
                '--go', paths['go'], '--javap', Path(paths['jdk']) / 'bin/javap', '--readelf', readelf]
    subprocess.run([str(arg) for arg in args], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'build', 'verify'])
    parser.add_argument('--apk', type=Path)
    parser.add_argument('--abi', default='arm64-v8a')
    parser.add_argument('--r8', type=Path)
    args = parser.parse_args()
    {'prepare': prepare, 'build': build, 'verify': lambda: verify(args.apk, args.abi, args.r8)}[args.action]()


if __name__ == '__main__':
    main()
