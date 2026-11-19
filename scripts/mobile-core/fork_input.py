"""Formal mobile input declarations; no build, publication or global tool install.

The common Git source is shared with the published CLI/OpenWrt input. Mobile
binding targets, tools and receipts are separate. An input declaration cannot
promote a diagnostic AAR/framework or a CLI receipt into App acceptance.
"""
import hashlib
import os
import json
from functools import lru_cache
from pathlib import Path
import re
import subprocess
from source_policy import verify_source_policy_files, verify_vendor_sources

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = Path(__file__).with_name('source-contract.json')


def require(condition, reason):
    if not condition:
        raise RuntimeError(reason)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False,
                      allow_nan=False).encode()


def decode(raw, maximum=65536):
    def pairs(rows):
        value = {}
        for key, item in rows:
            require(key not in value, 'duplicate mobile contract key')
            value[key] = item
        return value
    require(len(raw) <= maximum, 'mobile input contract is too large')
    return json.loads(raw, object_pairs_hook=pairs,
                      parse_constant=lambda _: (_ for _ in ()).throw(RuntimeError('nonfinite input')))


@lru_cache(maxsize=8)
def admit_declaration(raw):
    result = subprocess.run(['node', Path(__file__).with_name('source-admission.mjs')],
                            input=raw, capture_output=True, timeout=30)
    require(result.returncode == 0,
            result.stderr.decode(errors='replace').strip() or 'formal mobile source declaration rejected')
    return json.loads(result.stdout)


def validate(value):
    # Reuse only an identical complete declaration within this process.
    admit_declaration(canonical(value))
    return value


def load(path=CONTRACT):
    return validate(decode(Path(path).read_bytes()))


def git_environment(env=None):
    """Bind Git to the real checkout, not ambient repository/config overrides."""
    clean = {k: v for k, v in (os.environ if env is None else env).items() if not k.startswith('GIT_')}
    clean.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull,
                 GIT_NO_REPLACE_OBJECTS='1', GIT_TERMINAL_PROMPT='0')
    return clean


def require_git_root(source):
    actual = subprocess.check_output(['git', '-C', str(source), 'rev-parse', '--show-toplevel'],
                                     env=git_environment(), text=True, timeout=10).strip()
    require(Path(actual).resolve(strict=True) == Path(source).resolve(strict=True),
            'Git worktree differs from actual source root')


def git(source, *args):
    # Read-only source verification. Do not run the upstream build helper: it
    # chooses extra platforms/legacy variants and a git-describe version.
    require_git_root(source)
    return subprocess.check_output(['git', '-C', str(source), *args], env=git_environment(), text=True, timeout=30).strip()


def verify_source(source, value, *, generator_outputs=False):
    value = validate(value)
    source = Path(source).resolve(strict=True)
    require(git(source, 'cat-file', '-t', value['sourceTagObject']) == 'tag', 'annotated source tag required')
    require(git(source, 'rev-parse', value['sourceTag'] + '^{tag}') == value['sourceTagObject'] and
            git(source, 'rev-parse', value['sourceTagObject'] + '^{commit}') == value['sourceCommit'] and
            git(source, 'rev-parse', value['sourceCommit'] + '^{tree}') == value['sourceTree'],
            'mobile source tag/object/commit/tree differs')
    require(git(source, 'rev-parse', 'HEAD') == value['sourceCommit'] and
            not git(source, 'status', '--porcelain=v1', '--untracked-files=no'),
            'source checkout must be exact and clean before mobile bind')
    for name, key in [('go.mod', 'goModSha256'), ('go.sum', 'goSumSha256')]:
        require(sha((source / name).read_bytes()) == value[key], 'source module bytes differ')
    policy = value['sourcePolicy']
    module = (source / 'go.mod').read_text()
    directive = policy['goDirective']
    require(re.search(r'^module\s+' + re.escape(value['goModule']) + r'\s*$', module, re.M) and
            re.search(r'^go\s+' + re.escape(directive) + r'\s*$', module, re.M) and
            re.search(r'^\s*github\.com/sagernet/gomobile\s+' + re.escape(value['gomobileVersion']) + r'\s*$', module, re.M),
            'actual source Go/module/generator declaration differs')
    declared = value['declaredTagsSource']
    require(declared['path'] == 'cmd/internal/build_libbox/main.go' and
            sha((source / declared['path']).read_bytes()) == declared['sha256'], 'source-owned tags declaration differs')
    # Only the fixed producer view may contain its generator-owned build outputs.
    # The supplied source has no such exemption. Tracked changes always refuse.
    ignored = git(source, 'ls-files', '--others', '--ignored', '--exclude-standard').splitlines()
    # Go accepts Objective-C, C++, assembler, SWIG and Fortran as cgo inputs;
    # go:embed can consume arbitrary data extensions. No ignored file is an
    # admissible zero-overlay input in the isolated producer view.
    untracked = git(source, 'ls-files', '--others', '--exclude-standard').splitlines()
    unexpected = [name for name in ignored + untracked
                  if not (generator_outputs and name.startswith('build/'))]
    require(not unexpected, 'untracked ignored compiler/embed input is not a zero-overlay source')
    meta = policy['metacubex']
    prefix = meta['replacementPath'][2:]
    total_bytes = 0
    _, files, modes, trees = verify_vendor_sources(source, policy['vendors'], git)
    for name in ['go.mod', 'go.sum', meta['bindingPath'], meta['patchPath']]:
        path = source / name
        require(path.is_file() and not path.is_symlink() and path.resolve(strict=True).is_relative_to(source),
                'reviewed source input redirected/missing')
        require(path.stat().st_size <= 2 * 1024 * 1024, 'reviewed source input exceeds bound')
        total_bytes += path.stat().st_size
        require(total_bytes <= 32 * 1024 * 1024, 'reviewed source proof exceeds bound')
        files[name] = path.read_bytes()
    return {'schema': 'polaris-mobile-source-observation-v1', 'sourceCommit': value['sourceCommit'],
            'sourceTree': value['sourceTree'], 'sourceTag': value['sourceTag'],
            'sourceTagObject': value['sourceTagObject'], 'sourceOverlay': 0,
            'inputContractSha256': sha(canonical(value)), 'artifactAcceptance': False,
            'reviewedSource': verify_source_policy_files(policy, files, git(source, 'rev-parse', 'HEAD:' + prefix),
                                                         vendor_modes=modes, vendor_trees=trees)}


def binding_flags(authority, input_fingerprint):
    require(isinstance(input_fingerprint, str) and re.fullmatch('[0-9a-f]{64}', input_fingerprint),
            'actual binding input fingerprint required')
    return authority['linkerFlagsPrefix'] + input_fingerprint


def linker_flags(value, input_fingerprint):
    return binding_flags(admit_declaration(canonical(value)), input_fingerprint)


def declared_plan(platform, value):
    require(platform in ('android', 'apple'), 'unknown formal mobile platform')
    plan = admit_declaration(canonical(value))['platforms'][platform]
    facts = plan['facts']
    # Keep the original complete declaration, including its Python JSON values.
    return {**plan, 'facts': {'schema': facts['schema'], 'source': value, **facts}}


def bind_command(platform, value, gomobile, output, input_fingerprint, bootclasspath=None):
    require(platform in ('android', 'apple'), 'unknown formal mobile platform')
    authority = admit_declaration(canonical(value))
    inputs = {'gomobile': str(gomobile), 'output': str(output),
              'ldflags': binding_flags(authority, input_fingerprint)}
    if platform == 'android':
        require(bootclasspath is not None, 'explicit Android bootclasspath required')
        # Keep the original CLI spelling of admitted integers and floats.
        inputs['minimumAPI'] = str(value['android']['minimumAPI'])
        inputs['bootclasspath'] = str(bootclasspath)
    return [argument if isinstance(argument, str) else inputs[argument['input']]
            for argument in authority['platforms'][platform]['argv']]


def tool_environment(go, gomobile, gobind, cache, scratch, *, sdk=None, ndk=None,
                     jdk=None, developer=None):
    # Caller passes explicit scoped inputs. Do not inherit GOWORK/GOTOOLCHAIN,
    # ambient Android NDK selection, global GOFLAGS or an unrelated Go on PATH.
    import os
    env = git_environment()
    for key in ['GOWORK', 'GOFLAGS', 'GOTOOLCHAIN', 'GOROOT', 'GOOS', 'GOARCH', 'GOARM', 'GOARM64',
                'GOBIN', 'GOPATH', 'CGO_CFLAGS', 'CGO_CPPFLAGS', 'CGO_LDFLAGS', 'CC', 'CXX',
                'ANDROID_HOME', 'ANDROID_SDK_ROOT', 'ANDROID_NDK_HOME', 'ANDROID_NDK_ROOT',
                'DEVELOPER_DIR', 'SDKROOT', 'MACOSX_DEPLOYMENT_TARGET', 'IPHONEOS_DEPLOYMENT_TARGET']:
        env.pop(key, None)
    paths = [str(Path(p).parent) for p in [go, gomobile, gobind]]
    env.update(PATH=os.pathsep.join(paths + [env.get('PATH', '')]), GOWORK='off',
               GOFLAGS='-mod=readonly', GOTOOLCHAIN='local', CGO_ENABLED='1',
               GOCACHE=str(Path(cache) / 'go-build'), GOMODCACHE=str(Path(cache) / 'go-mod'),
               GOPATH=str(Path(cache) / 'go-path'),
               TMPDIR=str(scratch))
    if sdk is not None:
        require(ndk is not None and jdk is not None and developer is None, 'explicit Android tools required')
        # This SDK view has only the pinned NDK. Even a generator that prefers
        # SDK/ndk over ANDROID_NDK_HOME cannot fall back to an installed NDK28.
        env.update(ANDROID_HOME=str(sdk), ANDROID_SDK_ROOT=str(sdk),
                   ANDROID_NDK_HOME=str(ndk), ANDROID_NDK_ROOT=str(ndk), JAVA_HOME=str(jdk))
        env['PATH'] = str(Path(jdk) / 'bin') + os.pathsep + env['PATH']
    else:
        require(developer is not None and ndk is None and jdk is None, 'explicit Apple developer directory required')
        # Formal Apple inputs do not accept host pkg-config search extensions.
        # setup-python injects its own lib/pkgconfig directory into the job;
        # remove it from the actual compiler environment, not just cache facts.
        # Other ambient native overrides remain observed and disable reuse.
        env.pop('PKG_CONFIG_PATH', None)
        env['DEVELOPER_DIR'] = str(developer)
    return env


def show_plan():
    import argparse
    parser = argparse.ArgumentParser(description='Verify formal source and print a read-only mobile bind plan.')
    parser.add_argument('platform', choices=['android', 'apple'])
    for name in ('source', 'gomobile', 'output'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--bootclasspath', type=Path)
    args = parser.parse_args()
    value = load()
    observation = verify_source(args.source, value)
    plan = declared_plan(args.platform, value)
    facts = plan['facts']
    fingerprint = sha(canonical(facts))
    command = bind_command(args.platform, value, args.gomobile, args.output, fingerprint, args.bootclasspath)
    print(json.dumps({'schema': plan['schema'], 'facts': facts, 'fingerprint': fingerprint,
                      'command': command, **plan['summary'], 'sourceObservation': observation}, indent=2))


if __name__ == '__main__':
    show_plan()
