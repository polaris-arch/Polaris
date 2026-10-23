"""Bounded workflow diagnostics, never a runtime/cleanup/acceptance receipt."""
import argparse
import json
import os
from pathlib import Path
import re
import stat

SCHEMA = 'polaris-package-workflow-diagnostic-v1'
STEPS = ('checkout', 'admission', 'native_tools', 'test_tools', 'protoc',
         'core_go', 'core_receipts', 'transport', 'native_receipt', 'binding',
         'rpc_client', 'runtime', 'verification')
OUTCOMES = frozenset(('success', 'failure', 'cancelled', 'skipped'))
OUTPUTS = {'plain': 'linux-package-evidence/scope.json',
           'tun': 'linux-tun-evidence/scope.json'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def field(env, name, pattern):
    value = env.get(name, '')
    require(isinstance(value, str) and re.fullmatch(pattern, value) is not None,
            'Invalid diagnostic identity field: ' + name)
    return value


def build_scope(env, lane, kind):
    require(lane in OUTPUTS, 'Invalid diagnostic lane')
    require((lane == 'plain' and kind == 'both') or
            (lane == 'tun' and kind in ('deb', 'AppImage')), 'Invalid diagnostic carrier kind')
    context = {
        'sourceSha': field(env, 'GITHUB_SHA', r'[0-9a-f]{40}'),
        'runId': field(env, 'GITHUB_RUN_ID', r'[1-9][0-9]{0,19}'),
        'attempt': field(env, 'GITHUB_RUN_ATTEMPT', r'[1-9][0-9]{0,9}'),
        'job': field(env, 'GITHUB_JOB', r'[A-Za-z_][A-Za-z0-9_-]{0,99}'),
    }
    outcomes = {}
    for step in STEPS:
        value = env.get('DIAGNOSTIC_' + step.upper() + '_OUTCOME', '')
        require(isinstance(value, str) and (value == '' or value in OUTCOMES),
                'Invalid diagnostic outcome: ' + step)
        outcomes[step] = value or 'not-recorded'
    return {
        'schema': SCHEMA, 'scope': 'workflow-diagnostic-only',
        'acceptance': False, 'publicationEligible': False,
        'context': context, 'lane': lane, 'carrierKind': kind,
        'stepOutcomes': outcomes,
        'runtime': 'not-started' if outcomes['runtime'] == 'skipped' else 'attempted-or-unknown',
        'cleanup': 'not-assessed',
    }


def write_scope(root, scope):
    lane = scope['lane']
    require(lane in OUTPUTS, 'Invalid diagnostic lane')
    root = Path(root).absolute()
    # Check every existing ancestor rather than resolving through a symlink.
    for directory in reversed((root, *root.parents)):
        require(stat.S_ISDIR(directory.lstat().st_mode), 'Non-directory diagnostic root')
    output = root / OUTPUTS[lane]
    output.parent.mkdir(mode=0o700, exist_ok=True)
    require(stat.S_ISDIR(output.parent.lstat().st_mode), 'Linked diagnostic directory')
    data = (json.dumps(scope, sort_keys=True, indent=2) + '\n').encode('utf-8')
    require(len(data) <= 8192, 'Diagnostic scope too large')
    # Never overwrite original evidence; O_EXCL also rejects a dangling symlink.
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(data)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lane', choices=tuple(OUTPUTS), required=True)
    parser.add_argument('--kind', choices=('both', 'deb', 'AppImage'), required=True)
    args = parser.parse_args()
    write_scope(Path.cwd(), build_scope(os.environ, args.lane, args.kind))


if __name__ == '__main__':
    main()
