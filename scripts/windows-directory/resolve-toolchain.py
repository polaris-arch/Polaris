"""Resolve directory-test compiler policy, without running Go or selecting a source.

Go's `go` directive is a minimum, not evidence of a released binary's compiler.
Formal mode uses the existing release producer fact; candidate mode deliberately
chooses the source suggestion (or exact go minimum if no suggestion is present).
Only canonical patch-level stable versions are supported here, not Go's entire
version grammar. Language-only, prerelease, custom/default toolchains fail closed.
"""
import argparse
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'mobile-core'))
from source_policy import require, source_module_declarations


def version(value):
    require(isinstance(value, str) and re.fullmatch(r'1\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)', value),
            'canonical exact stable Go patch version required')
    return tuple(map(int, value.split('.')))


def resolve(raw, mode, source, contract=None, actual_go=None):
    require(mode in ('formal', 'candidate') and isinstance(source, str) and
            re.fullmatch('[0-9a-f]{40}', source), 'explicit mode and exact source required')
    directives, _, _ = source_module_declarations(raw)
    require(directives.get('module') == 'github.com/sagernet/sing-box', 'kernel module differs')
    minimum = directives.get('go')
    minimum_version = version(minimum)
    suggested = directives.get('toolchain')
    if suggested is not None:
        require(suggested.startswith('go'), 'standard Go toolchain required')
        require(version(suggested[2:]) >= minimum_version, 'toolchain is below Go minimum')
    if mode == 'formal':
        require(isinstance(contract, dict) and
                contract.get('schema') == 'polaris-box-release-consumer-contract-v1',
                'existing formal release contract required')
        win = contract['platforms']['win']
        require(win['sourceCommit'] == source, 'formal source differs')
        compiler = win['portablePlatformIdentity']['facts']['goVersion']
        require(isinstance(compiler, str) and compiler.startswith('go'), 'formal compiler fact required')
        require(version(compiler[2:]) >= minimum_version, 'formal compiler below Go minimum')
        require(suggested is None or version(compiler[2:]) >= version(suggested[2:]),
                'formal compiler conflicts with source suggestion')
        origin = 'formal-release-producer-fact'
    else:
        require(contract is None, 'candidate must not borrow formal compiler facts')
        compiler = suggested or 'go' + minimum
        origin = 'source-toolchain-suggestion' if suggested else 'source-exact-minimum-selection'
    if actual_go is not None:
        require(actual_go == compiler, 'actual Go compiler differs from selected exact compiler')
    return dict(validationMode=mode, kernelSource=source, goMinimum=minimum,
                sourceToolchain=suggested, selectedCompiler=compiler,
                selectionOrigin=origin, actualCompiler=actual_go)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel-root', type=Path, required=True)
    parser.add_argument('--mode', choices=['formal', 'candidate'], required=True)
    parser.add_argument('--source', required=True)
    parser.add_argument('--actual-go')
    parser.add_argument('--github-output', type=Path)
    args = parser.parse_args()
    contract = None
    if args.mode == 'formal':
        contract = json.loads((Path(__file__).resolve().parents[1] /
                               'desktop-core/release-contract.json').read_text(encoding='utf-8'))
    result = resolve((args.kernel_root / 'go.mod').read_bytes(), args.mode,
                     args.source, contract, args.actual_go)
    if args.github_output:
        with args.github_output.open('a', encoding='utf-8') as output:
            output.write('version=' + result['selectedCompiler'][2:] + '\n')
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    main()
