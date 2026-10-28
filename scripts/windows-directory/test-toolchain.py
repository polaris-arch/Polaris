import copy
import json
from pathlib import Path
import runpy
import tempfile
import subprocess
import sys
import unittest

HERE = Path(__file__).resolve().parent
resolve = runpy.run_path(str(HERE / 'resolve-toolchain.py'))['resolve']
SOURCE = 'a' * 40
MOD = b'module github.com/sagernet/sing-box\ngo 1.25.5\n'
CONTRACT = {'schema': 'polaris-box-release-consumer-contract-v1', 'platforms': {'win': {
    'sourceCommit': SOURCE, 'portablePlatformIdentity': {'facts': {'goVersion': 'go1.26.8'}}}}}


class ToolchainTests(unittest.TestCase):
    def test_formal_uses_producer_fact_not_minimum(self):
        row = resolve(MOD, 'formal', SOURCE, CONTRACT, 'go1.26.8')
        self.assertEqual(row['goMinimum'], '1.25.5')
        self.assertEqual(row['selectedCompiler'], 'go1.26.8')
        self.assertEqual(row['selectionOrigin'], 'formal-release-producer-fact')
        self.assertEqual(row['actualCompiler'], 'go1.26.8')

    def test_checked_in_formal_contract_without_toolchain(self):
        contract = json.loads((HERE.parent / 'desktop-core/historical-release-contract-577d4b9.json').read_text())
        source = contract['platforms']['win']['sourceCommit']
        self.assertEqual(source, '577d4b9a089b71d2f0a4ca334b87264b55c10478')
        self.assertEqual(resolve(MOD, 'formal', source, contract)['selectedCompiler'], 'go1.25.5')

    def test_current_upstream_formal_uses_exact_native_producer_fact(self):
        contract = json.loads((HERE.parent / 'desktop-core/release-contract.json').read_text())
        source = contract['sourceCommit']
        self.assertEqual(source, 'a4d0858605b51b7f5b3c29a9969a6f2c3ae5a0ae')
        mod = b'module github.com/sagernet/sing-box\ngo 1.27.0\ntoolchain go1.27.2\n'
        self.assertEqual(resolve(mod, 'formal', source, contract, 'go1.27.2')['selectedCompiler'], 'go1.27.2')
        for key, bad in [('sourceCommit', 'b' * 40), ('sourceTree', 'b' * 40),
                         ('repository', 'other/repo'), ('providerContract', 'invented')]:
            changed = copy.deepcopy(contract); changed[key] = bad
            with self.assertRaises(RuntimeError): resolve(mod, 'formal', source, changed)
        changed = copy.deepcopy(contract); changed['platforms']['win']['goVersion'] = 'go1.26.8'
        with self.assertRaises(RuntimeError): resolve(mod, 'formal', source, changed)

    def test_candidate_selection_is_not_release_provenance(self):
        row = resolve(MOD, 'candidate', SOURCE)
        self.assertEqual(row['selectionOrigin'], 'source-exact-minimum-selection')
        self.assertIsNone(row['actualCompiler'])
        self.assertEqual(resolve(MOD + b'toolchain go1.27.2\n', 'candidate', SOURCE)['selectedCompiler'], 'go1.27.2')

    def test_whitespace_comments_and_crlf(self):
        mod = b'\tmodule github.com/sagernet/sing-box // module\r\n go\t1.25.5 // minimum\r\n toolchain\tgo1.27.2 // suggestion\r\n'
        self.assertEqual(resolve(mod, 'candidate', SOURCE)['selectedCompiler'], 'go1.27.2')

    def test_ambiguous_or_unsupported_declarations_fail(self):
        bad = [b'go 1.25.5\n', b'toolchain go1.27.2\ntoolchain go1.27.2\n',
               b'toolchain go1.27\n', b'toolchain default\n', b'toolchain go1.27rc1\n',
               b'toolchain go1.27.2-custom\n', b'toolchain go1.24.9\n',
               b'toolchain "go1.27.2"\n', b'toolchain go1.27.2 extra\n',
               b'toolchain (\ngo1.27.2\n)\n', b'/* toolchain go1.27.2 */\n', b'\x00']
        for suffix in bad:
            with self.subTest(suffix=suffix), self.assertRaises(RuntimeError):
                resolve(MOD + suffix, 'candidate', SOURCE)
        for minimum in ['1.25', '1.25rc1', '1.025.5', '1.25.05', '2.25.5']:
            with self.subTest(minimum=minimum), self.assertRaises(RuntimeError):
                resolve(MOD.replace(b'1.25.5', minimum.encode()), 'candidate', SOURCE)
        for mod in [MOD.replace(b'go 1.25.5\n', b''), MOD.replace(b'sagernet/sing-box', b'other/module')]:
            with self.assertRaises(RuntimeError):
                resolve(mod, 'candidate', SOURCE)

    def test_wrong_modes_sources_and_compilers_fail(self):
        for mode, source, contract, actual in [
                ('auto', SOURCE, None, None), ('formal', 'b' * 40, CONTRACT, None),
                ('formal', SOURCE, None, None), ('candidate', SOURCE, CONTRACT, None),
                ('candidate', 'A' * 40, None, None), ('formal', SOURCE, CONTRACT, 'go1.25.5')]:
            with self.subTest(mode=mode, source=source, actual=actual), self.assertRaises(RuntimeError):
                resolve(MOD, mode, source, contract, actual)
        for compiler in ['go1.24.9', 'go1.26', 'go1.26.8-custom']:
            contract = copy.deepcopy(CONTRACT)
            contract['platforms']['win']['portablePlatformIdentity']['facts']['goVersion'] = compiler
            with self.assertRaises(RuntimeError):
                resolve(MOD, 'formal', SOURCE, contract)
        with self.assertRaises(RuntimeError):
            resolve(MOD + b'toolchain go1.27.2\n', 'formal', SOURCE, CONTRACT)

    def test_cli_output_and_failure_do_not_publish_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); (root / 'go.mod').write_bytes(MOD)
            output = root / 'github-output'
            args = [sys.executable, '-B', str(HERE / 'resolve-toolchain.py'), '--kernel-root', str(root),
                    '--mode', 'candidate', '--source', SOURCE, '--github-output', str(output)]
            result = subprocess.run(args, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(output.read_text(), 'version=1.25.5\n')
            self.assertIsNone(json.loads(result.stdout)['actualCompiler'])
            (root / 'go.mod').write_bytes(MOD + b'toolchain default\n')
            result = subprocess.run(args, text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(output.read_text(), 'version=1.25.5\n')


if __name__ == '__main__':
    unittest.main()
