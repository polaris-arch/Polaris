"""Bounded workflow diagnostics, never a runtime/cleanup/acceptance receipt."""
import argparse
import json
import importlib.util
import pwd
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


def write_scope(root, scope, *, output_relative=None):
    if output_relative is None:
        lane = scope['lane']
        require(lane in OUTPUTS, 'Invalid diagnostic lane')
        output_relative = OUTPUTS[lane]
    else:
        require(output_relative == AUTH_OUTPUT, 'Invalid diagnostic output')
    root = Path(root).absolute()
    # Check every existing ancestor rather than resolving through a symlink.
    for directory in reversed((root, *root.parents)):
        require(stat.S_ISDIR(directory.lstat().st_mode), 'Non-directory diagnostic root')
    output = root / output_relative
    output.parent.mkdir(mode=0o700, exist_ok=True)
    require(stat.S_ISDIR(output.parent.lstat().st_mode), 'Linked diagnostic directory')
    data = (json.dumps(scope, sort_keys=True, indent=2) + '\n').encode('utf-8')
    require(len(data) <= 8192, 'Diagnostic scope too large')
    # Never overwrite original evidence; O_EXCL also rejects a dangling symlink.
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(data)
    return output


AUTH_OUTPUT = 'linux-auth-preflight-evidence/groups.json'


def preflight_context(env, event):
    """Bind a diagnostic to original dispatch; this grants no credential authority."""
    require(type(event) is dict and type(event.get('inputs')) is dict, 'Invalid preflight event')
    inputs = event['inputs']
    require(inputs.get('linux_auth_preflight_only') is True or inputs.get('linux_auth_preflight_only') == 'true', 'Preflight not selected')
    require(env.get('GITHUB_EVENT_NAME') == 'workflow_dispatch', 'Invalid preflight event kind')
    require(env.get('GITHUB_REPOSITORY') == 'polaris-arch/Polaris' and
            env.get('GITHUB_REPOSITORY_ID') == '1347028220', 'Invalid preflight repository')
    repo = event.get('repository', {})
    require(repo.get('full_name') == 'polaris-arch/Polaris' and str(repo.get('id')) == '1347028220',
            'Invalid original repository')
    ref = field(env, 'GITHUB_REF', r'refs/heads/hot/app-runtime-consumer-20261020')
    require(env.get('PREFLIGHT_WORKTREE_CLEAN') == 'true', 'Preflight source tree changed')
    require(event.get('ref') in (ref, ref.removeprefix('refs/heads/')), 'Invalid original ref')
    require(env.get('GITHUB_WORKFLOW_REF') == 'polaris-arch/Polaris/.github/workflows/package.yml@' + ref,
            'Invalid preflight workflow')
    sha = field(env, 'GITHUB_SHA', r'[0-9a-f]{40}')
    require(inputs.get('linux_acceptance_candidate') == sha == env.get('PREFLIGHT_CHECKOUT_SHA'),
            'Invalid preflight exact source')
    require(env.get('GITHUB_JOB') == 'setup' and env.get('RUNNER_OS') == 'Linux' and
            env.get('RUNNER_ENVIRONMENT') == 'github-hosted', 'Invalid preflight runner')
    require(inputs.get('platform') == 'linux', 'Invalid preflight platform')
    for key in ('linux_package_acceptance', 'linux_tun_acceptance', 'native_validation',
                'light_build', 'skip_quality_gates'):
        require(inputs.get(key, False) is False or inputs.get(key, False) in ('false', ''),
                'Conflicting preflight input')
    for key in ('linux_tun_credential_authorization', 'windows_system_candidate', 'platforms',
                'core_candidate', 'core_bundle_artifact', 'core_wire_input_artifact',
                'core_wire_input_artifact_id', 'core_wire_input_artifact_digest'):
        require(inputs.get(key, '') == '', 'Conflicting preflight input')
    require(inputs.get('linux_tun_auth_mode', 'existing-session') == 'existing-session',
            'Conflicting preflight authentication mode')
    return {'sourceSha': sha, 'ref': ref, 'job': 'setup',
            'runId': field(env, 'GITHUB_RUN_ID', r'[1-9][0-9]{0,19}'),
            'attempt': field(env, 'GITHUB_RUN_ATTEMPT', r'[1-9][0-9]{0,9}')}


def build_auth_preflight(env, event):
    context = preflight_context(env, event)
    uid = os.getuid()
    require(type(uid) is int and uid > 0, 'Preflight requires ordinary account')
    user = pwd.getpwuid(uid)
    require(user.pw_uid == uid and len([u for u in pwd.getpwall() if
            u.pw_uid == uid or u.pw_name == user.pw_name]) == 1,
            'Preflight account identity not unique')
    spec = importlib.util.spec_from_file_location('preflight_root_contract', Path(__file__).with_name('root_controller.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    groups = module.validate_group_observation(module.observe_existing_admin_groups(user))
    return {'schema': 'polaris-linux-auth-preflight-diagnostic-v1',
            'scope': 'read-only-account-group-diagnostic-not-credential-authority',
            'acceptance': False, 'publicationEligible': False, 'context': context,
            'runtime': 'not-started', 'cleanup': 'not-assessed', 'currentUid': uid,
            'groupObservation': groups}


def run_auth_preflight(env, root):
    # The original event is data only; never export its inputs or account records.
    with open(env['GITHUB_EVENT_PATH'], 'rb') as stream:
        raw = stream.read(65537)
    require(len(raw) <= 65536, 'Preflight event too large')
    value = build_auth_preflight(env, json.loads(raw))
    write_scope(root, value, output_relative=AUTH_OUTPUT)
    return 0 if value['groupObservation']['groupRequirementSatisfied'] is True else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lane', choices=tuple(OUTPUTS))
    parser.add_argument('--kind', choices=('both', 'deb', 'AppImage'))
    parser.add_argument('--auth-preflight-only', action='store_true')
    args = parser.parse_args()
    if args.auth_preflight_only:
        require(args.lane is None and args.kind is None, 'Conflicting diagnostic mode')
        try:
            status = run_auth_preflight(os.environ, Path.cwd())
        except Exception:
            # Never print original event/NSS exceptions or account records.
            raise SystemExit('AUTH_PREFLIGHT_INVALID') from None
        raise SystemExit(status)
    require(args.lane is not None and args.kind is not None, 'Missing diagnostic lane/kind')
    write_scope(Path.cwd(), build_scope(os.environ, args.lane, args.kind))


if __name__ == '__main__':
    main()
