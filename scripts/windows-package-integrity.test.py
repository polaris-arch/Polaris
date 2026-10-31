#!/usr/bin/env python3
"""Inert fixtures only: actual ZIP reads; NSIS parser/registration fixtures.
No Windows installer, product executable, signature or installation is simulated
as successful native acceptance.
"""
import copy
from contextlib import contextmanager
import importlib.util
import hashlib
import io
import json
import os
from types import SimpleNamespace
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import time
import tempfile
import unittest
from unittest.mock import patch
import warnings
import zipfile

os.sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('windows_integrity', ROOT / 'scripts/windows-package-integrity.py')
w = importlib.util.module_from_spec(spec); spec.loader.exec_module(w)


class Integrity(unittest.TestCase):
    @unittest.skipUnless(os.name == 'nt', 'native Windows stat/fstat projection required')
    def test_formal_asset_windows_opened_identity_and_same_view_guards(self):
        # Real ZIP and actual product reader; projections only exercise the
        # retained-object guards, never simulate a native installer or policy.
        for fault in ['none', 'foreign inode', 'opened size', 'fd ctime', 'fd mtime', 'fd links']:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                artifact = Path(tmp) / 'formal.zip'
                bodies = {'application/polaris.exe': b'inert ordinary App',
                          '_up_/resources/win/polaris-helper.exe': b'inert ordinary Helper'}
                with zipfile.ZipFile(artifact, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
                    for name, body in bodies.items(): archive.writestr(name, body)
                original = os.fstat; calls = 0
                def projected(fd):
                    nonlocal calls
                    calls += 1
                    value = original(fd)
                    fields = {name: getattr(value, name) for name in
                              ['st_dev', 'st_ino', 'st_mode', 'st_nlink', 'st_size', 'st_mtime_ns', 'st_ctime_ns']}
                    fields['st_ctime_ns'] += 100
                    if fault == 'foreign inode': fields['st_ino'] += 1
                    if fault == 'opened size': fields['st_size'] += 1
                    if calls == 2:
                        field = {'fd ctime': 'st_ctime_ns', 'fd mtime': 'st_mtime_ns', 'fd links': 'st_nlink'}.get(fault)
                        if field: fields[field] += 1
                    return SimpleNamespace(**fields)
                with patch.object(os, 'fstat', side_effect=projected):
                    if fault == 'none':
                        result = w.final.reject_local_selection(artifact)
                        self.assertEqual(result['artifactSha256'], hashlib.sha256(artifact.read_bytes()).hexdigest())
                        self.assertEqual({row['member']: row['sha256'] for row in result['checkedExecutables']},
                                         {name: hashlib.sha256(body).hexdigest() for name, body in bodies.items()})
                    else:
                        message = 'replaced before open' if fault in ['foreign inode', 'opened size'] else 'changed during actual object check'
                        with self.assertRaisesRegex(ValueError, message): w.final.reject_local_selection(artifact)
                self.assertEqual(calls, 1 if fault in ['foreign inode', 'opened size'] else 2)

    def fixture(self, root):
        (root / 'src-tauri').mkdir()
        (root / 'src-tauri/tauri.windows.conf.json').write_bytes((ROOT / 'src-tauri/tauri.windows.conf.json').read_bytes())
        (root / 'src-tauri/core-manifest.json').write_bytes(b'{"declaration":"fixture"}\n')
        for name, content in {
            'LICENSE': b'MIT notice', 'NOTICE': b'Core source offer', 'THIRD-PARTY-LICENSES.md': b'Complete licenses',
            'resources/win/core-manifest.json': b'{"compiled":"selected fixture pin"}\n',
            'resources/win/polaris-helper.exe': b'current helper', 'resources/win/polaris-cleaner.exe': b'current cleaner',
            'resources/win/sing-box.exe': b'current core', 'resources/win/libcronet.dll': b'current cronet',
            # Legitimate source/legal/runtime scripts must survive exact inventory.
            'resources/win/core-release/source/runtime.sh': b'#!/bin/sh\n: legal archived source\n',
            'resources/win/core-release/LICENSE': b'GPLv3 full text fixture',
            'resources/dashboard/index.html': b'<script src="assets/current.js"></script>',
            'resources/dashboard/assets/current.js': b'current dashboard',
            'resources/dashboard/licenses/font.txt': b'font notice',
            'resources/data/geosite.srs': b'SRS current bytes',
        }.items():
            p = root / name; p.parent.mkdir(parents=True, exist_ok=True); p.write_bytes(content)
        # The real platform declaration now carries one independent original
        # archive and inventory. These inert members are data only and cannot
        # satisfy any compiled installation/admission or execute as a kernel.
        row = json.loads((ROOT / 'scripts/desktop-core/release-contract.json').read_bytes())['platforms']['win']
        archive_name = row['archive']['name']
        archive_root = row['archive']['root']
        output = io.BytesIO()
        members = {'sing-box.exe': b'current core', 'libcronet.dll': b'current cronet', 'LICENSE': b'GPLv3 fixture'}
        with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
            for name, body in members.items(): archive.writestr(archive_root + '/' + name, body)
        archive_bytes = output.getvalue()
        recovery = root / 'resources/core-recovery/win'
        recovery.mkdir(parents=True)
        (recovery / archive_name).write_bytes(archive_bytes)
        (recovery / 'inventory.json').write_text(json.dumps({
            'schema': 'polaris-core-recovery-inventory-v1', 'platform': 'win',
            'contractSha256': hashlib.sha256(b'inert fixture contract, no execution policy').hexdigest(),
            'archive': {'name': archive_name, 'bytes': len(archive_bytes), 'sha256': hashlib.sha256(archive_bytes).hexdigest(), 'root': archive_root, 'format': 'zip'},
            'memberBytes': {name: len(body) for name, body in members.items()},
        }))
        resources, sources = w.source_inventory(root)
        return {'resources': resources, 'sources': sources}, b'MZ inert App ' + w.final.BUNDLE_UNK + b' ' + (root / 'resources/win/core-manifest.json').read_bytes()

    def zip(self, root, expected, state, app, fault=None):
        archive = root / 'carrier.zip'
        payload = {name[5:] if name.startswith('_up_/') else name: Path(source).read_bytes() for name, source in state['sources'].items()}
        payload.update({'polaris.exe': app, 'portable.marker': w.MARKER})
        if fault: fault(payload)
        with warnings.catch_warnings():
            warnings.simplefilter('ignore', UserWarning)
            with zipfile.ZipFile(archive, 'w') as z:
                for name, content in payload.items(): z.writestr(name, content)
        return archive

    def test_real_zip_complete_legal_resource_positive(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); state, app = self.fixture(root)
            expected = w.carrier_inventory(state, app, 'zip')
            archive = self.zip(root, expected, state, app)
            self.assertEqual(w.zip_inventory(archive, expected), expected)
            self.assertIn('resources/win/core-release/source/runtime.sh', expected)
            self.assertIn('core-manifest.json', expected)
            self.assertEqual(expected['polaris.exe'], w.byte_facts(app))
            self.assertNotEqual(w.carrier_inventory(state, app, 'nsis')['$R6/polaris.exe'], expected['polaris.exe'])

    def test_zip_full_set_and_every_byte_negative_matrix(self):
        mutations = {
            'residual script': lambda p: p.update({'old-debug.ps1': b'old'}),
            'residual screenshot': lambda p: p.update({'resources/dashboard/screenshot.png': b'old'}),
            'extra empty dir': lambda p: p.update({'unknown/': b''}),
            'missing manifest': lambda p: p.pop('resources/win/core-manifest.json'),
            'missing source archive': lambda p: p.pop('resources/win/core-release/source/runtime.sh'),
            'missing cleaner': lambda p: p.pop('resources/win/polaris-cleaner.exe'),
            'old App new name': lambda p: p.update({'polaris.exe': b'old app renamed current'}),
            'old helper new name': lambda p: p.update({'resources/win/polaris-helper.exe': b'old helper new'}),
            'same-size stale core': lambda p: p.update({'resources/win/sing-box.exe': b'x' * len(p['resources/win/sing-box.exe'])}),
            'stale Cronet': lambda p: p.update({'resources/win/libcronet.dll': b'old-old cronet'}),
            'stale dashboard': lambda p: p.update({'resources/dashboard/assets/current.js': b'obsolete dashboard'}),
            'stale rules': lambda p: p.update({'resources/data/geosite.srs': b'SRS old-old bytes'}),
            'empty license': lambda p: p.update({'LICENSE': b''}),
            'stale marker': lambda p: p.update({'portable.marker': b'wrong marker'}),
            'case alias': lambda p: p.update({'POLARIS.EXE': p['polaris.exe']}),
            'trailing dot alias': lambda p: p.update({'polaris.exe.': p['polaris.exe']}),
            'file directory alias': lambda p: p.update({'resources': b'file'}),
            'backslash alias': lambda p: p.update({'resources\\win\\extra': b'file'}),
            'repeated separator': lambda p: p.update({'resources//win/extra': b'file'}),
            'path escape': lambda p: p.update({'../outside': b'file'}),
        }
        for name, fault in mutations.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); state, app = self.fixture(root); expected = w.carrier_inventory(state, app, 'zip')
                archive = self.zip(root, expected, state, app, fault)
                with self.assertRaises(ValueError): w.zip_inventory(archive, expected)

    def test_zip_duplicate_member_and_symlink(self):
        for fault in ['duplicate', 'symlink', 'fifo']:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); state, app = self.fixture(root); expected = w.carrier_inventory(state, app, 'zip')
                archive = self.zip(root, expected, state, app)
                with warnings.catch_warnings():
                    warnings.simplefilter('ignore', UserWarning)
                    with zipfile.ZipFile(archive, 'a') as z:
                        if fault == 'duplicate': z.writestr('polaris.exe', app)
                        else:
                            info = zipfile.ZipInfo('unexpected-link')
                            info.external_attr = (stat.S_IFLNK if fault == 'symlink' else stat.S_IFIFO) << 16
                            z.writestr(info, b'polaris.exe')
                with self.assertRaises(ValueError): w.zip_inventory(archive, expected)

    def test_installer_candidate_identity_and_count(self):
        cases = [[], ['Polaris_0.9.0_x64-setup.exe'], ['Polaris_1.0.0_x64-setup.exe', 'Polaris_0.9.0_x64-setup.exe'],
                 ['Polaris_1.0.0_arm64-setup.exe']]
        for names in cases:
            with self.subTest(names=names), tempfile.TemporaryDirectory() as tmp:
                directory = Path(tmp)
                for name in names: (directory / name).write_bytes(b'inert installer')
                with self.assertRaises(ValueError): w.unique_installer(directory, '1.0.0')
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp); current = directory / 'Polaris_1.0.0_x64-setup.exe'; current.write_bytes(b'inert')
            self.assertEqual(w.unique_installer(directory, '1.0.0'), current)

    def test_fresh_portable_staging_never_overlays_residual_namespace(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); state, app = self.fixture(root); state['owner'] = str(root)
            legacy = root / 'staging-portable'; legacy.mkdir(); (legacy / 'old.ps1').write_bytes(b'old')
            archive = root / 'new.zip'; w.build_portable(root, state, app, archive)
            with zipfile.ZipFile(archive) as z: self.assertNotIn('old.ps1', z.namelist())
            self.assertEqual((legacy / 'old.ps1').read_bytes(), b'old')
            with self.assertRaises(FileExistsError): w.build_portable(root, state, app, archive)

    def test_prepare_quarantines_only_fixed_old_namespaces(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); state, app = self.fixture(root)
            (root / 'src-tauri/tauri.conf.json').write_text('{"version":"1.0.0"}')
            for path in ['target/release/bundle/nsis', 'dist-win']:
                (root / path).mkdir(parents=True); (root / path / 'old-setup.exe').write_bytes(b'old')
            sentinel = root / 'target/release/keep-me'; sentinel.write_bytes(b'untouched')
            env = root / 'env'
            with patch.dict(os.environ, {'GITHUB_ENV': str(env)}), patch.object(w, 'find_sevenzip', return_value=('7z', '25.01')), \
                 patch.object(w, 'verified_sources', return_value=(state['resources'], state['sources'])), \
                 patch.object(w.subprocess, 'check_output', return_value='a' * 40 + '\n'):
                w.prepare(root)
            owner = next((root / 'target/release').glob('.polaris-package-*'))
            self.assertEqual((owner / 'previous-nsis/old-setup.exe').read_bytes(), b'old')
            self.assertEqual((owner / 'previous-dist-win/old-setup.exe').read_bytes(), b'old')
            self.assertEqual(list((root / 'dist-win').iterdir()), [])
            self.assertEqual(list((root / 'target/release/bundle/nsis').iterdir()), [])
            self.assertEqual(sentinel.read_bytes(), b'untouched')
            self.assertIn(str(owner / 'state.json'), env.read_text())

    def test_source_and_zip_mapping_share_one_inventory(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); state, app = self.fixture(root)
            nsis = w.carrier_inventory(state, app, 'nsis'); portable = w.carrier_inventory(state, app, 'zip')
            for name, row in state['resources'].items():
                self.assertEqual(nsis['$R6/' + name], row)
                self.assertEqual(portable[name[5:] if name.startswith('_up_/') else name], row)
            (root / 'resources/win/polaris-helper.exe').write_bytes(b'old helper same path')
            changed, _ = w.source_inventory(root)
            self.assertNotEqual(changed, state['resources'])

    def test_compiled_pin_requires_exact_current_manifest_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); state, app = self.fixture(root)
            binary = root / 'target/release/polaris.exe'; binary.parent.mkdir(parents=True); binary.write_bytes(app)
            self.assertEqual(w.compiled_app(root)[1], app)
            for stale in [b'old app without current pin', app + (root / 'resources/win/core-manifest.json').read_bytes()]:
                binary.write_bytes(stale)
                with self.assertRaises(ValueError): w.compiled_app(root)
            binary.write_bytes(app)
            (root / 'resources/win/core-manifest.json').write_bytes(b'changed selected identity')
            with self.assertRaises(ValueError): w.compiled_app(root)

    def test_source_link_and_hardlink_rejection(self):
        if os.name == 'nt': self.skipTest('Unix fixture link creation only')
        for fault in ['symlink-file', 'symlink-dir', 'hardlink']:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); state, app = self.fixture(root)
                if fault == 'symlink-file':
                    p = root / 'resources/win/sing-box.exe'; p.unlink(); p.symlink_to(root / 'LICENSE')
                elif fault == 'symlink-dir':
                    (root / 'resources/win/alias').symlink_to(root / 'resources/data', target_is_directory=True)
                else: os.link(root / 'LICENSE', root / 'resources/win/hardlink')
                with self.assertRaises(ValueError): w.source_inventory(root)

    def test_cargo_known_pair_stages_private_exact_single_link_without_changing_aliases(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); _, app = self.fixture(root)
            binary = root / 'target/release/polaris.exe'; binary.parent.mkdir(parents=True); binary.write_bytes(app)
            alias = binary.parent / 'deps/polaris.exe'; alias.parent.mkdir(); os.link(binary, alias)
            before = w.cargo_app_identity(root)[1]
            stage = root / 'private'; stage.mkdir()
            staged, captured = w.compiled_app(root, staging=stage)
            self.assertEqual(captured, app); self.assertEqual(staged.read_bytes(), app)
            self.assertEqual(staged.stat().st_nlink, 1)
            self.assertNotEqual(staged.stat().st_ino, binary.stat().st_ino)
            self.assertEqual(w.cargo_app_identity(root)[1], before)
            self.assertTrue(alias.samefile(binary))
            with self.assertRaises(ValueError): w.regular(binary)
            with self.assertRaises(FileExistsError): w.compiled_app(root, staging=stage)

    def test_cargo_unregistered_or_surplus_links_fail_closed(self):
        for fault in ['unknown-alias', 'different-deps', 'third-link']:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); _, app = self.fixture(root)
                binary = root / 'target/release/polaris.exe'; binary.parent.mkdir(parents=True); binary.write_bytes(app)
                deps = binary.parent / 'deps'; deps.mkdir()
                if fault == 'unknown-alias': os.link(binary, root / 'unknown.exe')
                elif fault == 'different-deps':
                    os.link(binary, root / 'unknown.exe'); (deps / 'polaris.exe').write_bytes(app)
                else:
                    os.link(binary, deps / 'polaris.exe'); os.link(binary, root / 'third.exe')
                with self.assertRaises((ValueError, FileNotFoundError)): w.compiled_app(root)

    def test_cargo_change_during_capture_rejected_before_staging(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); _, app = self.fixture(root)
            binary = root / 'target/release/polaris.exe'; binary.parent.mkdir(parents=True); binary.write_bytes(app)
            original = w.cargo_app_identity; calls = 0
            def changed(value):
                nonlocal calls
                calls += 1
                if calls == 2: binary.write_bytes(app + b'changed')
                return original(value)
            with patch.object(w, 'cargo_app_identity', side_effect=changed), \
                 self.assertRaisesRegex(ValueError, 'Cargo App path changed while reading'):
                w.compiled_app(root)
            self.assertEqual(calls, 2)

    def test_cargo_fd_projection_and_same_view_change_guards(self):
        # Model Windows' unlike stat/fstat projections on every platform while
        # retaining a real inert file read and both same-view change guards.
        for fault in ['equivalent projection', 'foreign inode', 'fd ctime change', 'fd mtime change']:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); _, app = self.fixture(root)
                binary = root / 'target/release/polaris.exe'; binary.parent.mkdir(parents=True); binary.write_bytes(app)
                original = os.fstat; calls = 0
                def projected(fd):
                    nonlocal calls
                    calls += 1
                    value = original(fd)
                    fields = {name: getattr(value, name) for name in
                              ['st_dev', 'st_ino', 'st_mode', 'st_nlink', 'st_size', 'st_mtime_ns', 'st_ctime_ns']}
                    fields['st_mode'] &= ~0o111
                    fields['st_ctime_ns'] += 100
                    if fault == 'foreign inode': fields['st_ino'] += 1
                    if calls == 2 and fault == 'fd ctime change': fields['st_ctime_ns'] += 1
                    if calls == 2 and fault == 'fd mtime change': fields['st_mtime_ns'] += 1
                    return SimpleNamespace(**fields)
                with patch.object(w.os, 'fstat', side_effect=projected):
                    if fault == 'equivalent projection': self.assertEqual(w.compiled_app(root)[1], app)
                    else:
                        message = 'replaced before read' if fault == 'foreign inode' else 'changed while reading'
                        with self.assertRaisesRegex(ValueError, message): w.compiled_app(root)
                self.assertEqual(calls, 1 if fault == 'foreign inode' else 2)

    def test_normal_tauri_entry_is_resolved_before_version_probe(self):
        entry = str(Path(tempfile.gettempdir()) / 'official-cli/tauri.cmd')
        with patch.object(w.shutil, 'which', return_value=entry) as find:
            self.assertEqual(w.tauri_command(), entry); find.assert_called_once_with('tauri')
        with patch.object(w.shutil, 'which', return_value=None), self.assertRaises(ValueError): w.tauri_command()
        if os.name == 'nt':
            with patch.object(w.shutil, 'which', return_value='tauri.ps1'), self.assertRaises(ValueError): w.tauri_command()

    def test_tauri_probe_runs_the_resolved_entry_and_rejects_other_versions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); scripts = root / 'scripts'; scripts.mkdir()
            declaration = scripts / 'tauri-cli.version'; declaration.write_text(w.final.TAURI_CLI_VERSION + '\n')
            entry = str(root / 'official-tauri.cmd')
            with patch.object(w.shutil, 'which', return_value=entry), \
                 patch.object(w.subprocess, 'check_output', return_value='tauri-cli ' + w.final.TAURI_CLI_VERSION + '\n') as probe:
                self.assertEqual(w.tauri_identity(root), {'entry': entry, 'version': 'tauri-cli ' + w.final.TAURI_CLI_VERSION})
                probe.assert_called_once_with([entry, '--version'], timeout=30, text=True)
            for output in ['tauri-cli 2.12.3', 'tauri-cli 2.12.1\nextra', '2.12.1']:
                with self.subTest(output=output), patch.object(w.shutil, 'which', return_value=entry), \
                     patch.object(w.subprocess, 'check_output', return_value=output), self.assertRaises(ValueError):
                    w.tauri_identity(root)
            declaration.write_text('2.12.3\n')
            with patch.object(w.shutil, 'which', return_value=entry), \
                 patch.object(w.subprocess, 'check_output', return_value='tauri-cli 2.12.3'), self.assertRaises(ValueError):
                w.tauri_identity(root)

    def listing(self, names):
        return 'Type = Nsis\nSolid = +\n----------\n' + '\n\n'.join('Path = ' + name + '\nSize = ' + ('' if size is None else str(size)) for name, size in names) + '\n'

    def test_nsis_reader_listing_parser_positive_and_alias_matrix(self):
        rows = [('polaris.exe', 100), ('_up_\\resources\\win\\sing-box.exe', 200), ('$PLUGINSDIR\\System.dll', 50)]
        self.assertEqual(w.nsis_listing(self.listing(rows)), [('polaris.exe', 100), ('_up_/resources/win/sing-box.exe', 200), ('$PLUGINSDIR/System.dll', 50)])
        for rows in [[('polaris.exe', 1), ('polaris.exe', 1)], [('polaris.exe', 1), ('POLARIS.EXE', 1)],
                     [('../escape', 1)], [('resources//win/core', 1)], [('polaris.exe.', 1)], [('nul.exe', 1)],
                     [('resources', 1), ('resources/win/core', 2)], [('huge', w.MAX_FILE + 1)]]:
            with self.subTest(rows=rows), self.assertRaises(ValueError): w.nsis_listing(self.listing(rows))
        with self.assertRaises(ValueError): w.nsis_listing('Type = zip\n----------\nPath = polaris.exe\nSize = 1')

    def test_nsis_exact_payload_and_installer_owned_namespace_matrix(self):
        # Parser/registration fixture: no real NSIS or Windows native proof.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); state, app = self.fixture(root); expected = w.carrier_inventory(state, app, 'nsis')
            plugins = {'$PLUGINSDIR/System.dll': w.byte_facts(b'toolchain plugin')}
            original_plugins = copy.deepcopy(plugins)
            cleaner = expected['$R6/_up_/resources/win/polaris-cleaner.exe']
            current = {**expected, **plugins, '$PLUGINSDIR/polaris-cleaner.exe': cleaner, 'uninstall.exe': w.byte_facts(b'generated stub')}
            nested = {**plugins, '$PLUGINSDIR/polaris-cleaner.exe': cleaner}
            mutations = {
                'valid': lambda a, b: None,
                'old App': lambda a, b: a.update({'$R6/polaris.exe': w.byte_facts(b'old App')}),
                'missing helper': lambda a, b: a.pop('$R6/_up_/resources/win/polaris-helper.exe'),
                'old pin': lambda a, b: a.update({'$R6/_up_/resources/win/core-manifest.json': w.byte_facts(b'old pin')}),
                'extra resource': lambda a, b: a.update({'$R6/_up_/resources/win/debug.ps1': w.byte_facts(b'old')}),
                'wrong App directory': lambda a, b: a.update({'other/polaris.exe': a.pop('$R6/polaris.exe')}),
                'bare App root': lambda a, b: a.update({'polaris.exe': a.pop('$R6/polaris.exe')}),
                'other variable root': lambda a, b: a.update({'$R7/polaris.exe': a.pop('$R6/polaris.exe')}),
                'extra App': lambda a, b: a.update({'backup/polaris.exe': a['$R6/polaris.exe']}),
                'wrong plugin': lambda a, b: a.update({'$PLUGINSDIR/System.dll': w.byte_facts(b'other plugin')}),
                'renamed plugin': lambda a, b: a.update({'$PLUGINSDIR/other.dll': w.byte_facts(b'other')}),
                'missing installer cleaner': lambda a, b: a.pop('$PLUGINSDIR/polaris-cleaner.exe'),
                'old installer cleaner': lambda a, b: a.update({'$PLUGINSDIR/polaris-cleaner.exe': w.byte_facts(b'old cleaner')}),
                'missing uninstall cleaner': lambda a, b: b.pop('$PLUGINSDIR/polaris-cleaner.exe'),
                'old uninstall cleaner': lambda a, b: b.update({'$PLUGINSDIR/polaris-cleaner.exe': w.byte_facts(b'old cleaner')}),
                'extra uninstaller member': lambda a, b: b.update({'$PLUGINSDIR/debug.ps1': w.byte_facts(b'old')}),
            }
            errors = {
                'old App': 'final member SHA/size differs:',
                'missing helper': 'final member set differs;',
                'old pin': 'final member SHA/size differs:',
                'extra resource': 'unregistered NSIS installer member',
                'wrong App directory': 'unregistered NSIS installer member',
                'bare App root': 'unregistered NSIS installer member',
                'other variable root': 'unregistered NSIS installer member',
                'extra App': 'unregistered NSIS installer member',
                'wrong plugin': 'NSIS plugin differs from selected toolchain:',
                'renamed plugin': 'unregistered NSIS installer member',
                'missing installer cleaner': 'installer cleaner missing',
                'old installer cleaner': 'NSIS plugin differs from selected toolchain:',
                'missing uninstall cleaner': 'unregistered NSIS uninstaller member',
                'old uninstall cleaner': 'embedded uninstall cleaner differs',
                'extra uninstaller member': 'unregistered NSIS uninstaller member',
            }
            for name, mutate in mutations.items():
                with self.subTest(name=name):
                    actual, inner = copy.deepcopy(current), copy.deepcopy(nested); mutate(actual, inner)
                    with patch.object(w, 'plugin_inventory', return_value=plugins), \
                         patch.object(w, 'unpack_nsis', side_effect=[(actual, 'inert listing'), (inner, 'inert nested listing')]) as unpack:
                        if name == 'valid':
                            receipt = w.nsis_inventory(root, 'reader', root / 'inert.exe', expected, root)
                            self.assertEqual(receipt['members'], current)
                        else:
                            with self.assertRaisesRegex(ValueError, errors[name]):
                                w.nsis_inventory(root, 'reader', root / 'inert.exe', expected, root)
                        self.assertEqual(plugins, original_plugins)
                        self.assertEqual(unpack.call_count, 2 if name in ['valid', 'missing uninstall cleaner',
                                                                         'old uninstall cleaner', 'extra uninstaller member'] else 1)

    def test_generated_nsis_script_binds_all_current_sources(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); state, app = self.fixture(root); state['version'] = '1.0.0'
            release = root / 'target/release'; release.mkdir(parents=True); (release / 'polaris.exe').write_bytes(app)
            script = release / 'nsis/x64/installer.nsi'; script.parent.mkdir(parents=True)
            text = ('!define VERSION "1.0.0"\n!define MAINBINARYNAME "polaris"\n'
                    + '!define MAINBINARYSRCPATH "' + str(release / 'polaris.exe') + '"\nSection Install\n'
                    + '  File "${MAINBINARYSRCPATH}"\n'
                    + ''.join('  File /a "/oname=' + name + '" "' + source + '"\n' for name, source in state['sources'].items())
                    + 'SectionEnd\n')
            script.write_text(text); w.verify_generated_script(root, state)
            deps = release / 'deps'; deps.mkdir(); os.link(release / 'polaris.exe', deps / 'polaris.exe')
            w.verify_generated_script(root, state)
            for mutation in [text.replace('VERSION "1.0.0"', 'VERSION "0.9.0"'),
                             text.replace('  File "${MAINBINARYSRCPATH}"', '  File "${MAINBINARYSRCPATH}"\n  File "${MAINBINARYSRCPATH}"'),
                             text.replace('SectionEnd', '  File "old.exe"\nSectionEnd'),
                             text.replace('/oname=_up_/LICENSE', '/oname=_up_/LICENSE.')]:
                script.write_text(mutation)
                with self.assertRaises(ValueError): w.verify_generated_script(root, state)

    @contextmanager
    def inert_reader(self, listing, payloads, *, stderr_bytes=0, delay=0):
        # Exercise real pipe threads, bounded parent writes, exits and joins.
        # These children emit controlled bytes; none is a 7z/NSIS implementation.
        popen = subprocess.Popen; children = []; commands = []
        def launch(argv, **kwargs):
            commands.append(argv)
            payload = listing if argv[1] == 'l' else payloads[argv[-1]]
            if isinstance(payload, int):
                producer = 'remaining=' + str(payload) + "\nwhile remaining:\n n=min(65536,remaining);sys.stdout.buffer.write(b'x'*n);sys.stdout.buffer.flush();remaining-=n\n"
            else:
                producer = 'sys.stdout.buffer.write(' + repr(payload.encode() if isinstance(payload, str) else payload) + ');sys.stdout.buffer.flush()\n'
            program = ('import sys,time\ntime.sleep(' + repr(delay) + ')\n'
                       + 'sys.stderr.buffer.write(b"e"*' + str(stderr_bytes) + ');sys.stderr.buffer.flush()\n' + producer)
            child = popen([sys.executable, '-u', '-c', program], **kwargs); children.append(child); return child
        with patch.object(w.subprocess, 'Popen', side_effect=launch):
            try: yield children, commands
            finally:
                self.assertTrue(all(child.poll() is not None for child in children), 'every exact owned child must have exited/joined')

    def test_static_confs_contract_follows_actual_producer_and_complete_zip(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); self.fixture(root)
            workflow = (ROOT / '.github/workflows/package.yml').read_text(encoding='utf-8')
            source = Path(w.__file__).read_text(encoding='utf-8')
            result = w.contract(root, source_text=source, workflow_text=workflow)
            self.assertEqual(result['legalFiles'], ['LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md'])
            self.assertEqual(result['ordinaryWindowsCommands'], ['prepare', 'assemble'])
            # Eager containers/first operands still perform the real check.
            gate = '    zip_inventory(destination, expected)\n'
            for eager_form in ['[zip_inventory(destination, expected)]',
                               '(zip_inventory(destination, expected), None)',
                               "{'checked': zip_inventory(destination, expected)}",
                               'zip_inventory(destination, expected) and None']:
                changed = source.replace(gate, '    ' + eager_form + '\n')
                self.assertNotEqual(changed, source)
                self.assertEqual(w.contract(root, source_text=changed, workflow_text=workflow), result)
            mutations = [
                source.replace('        assemble_carriers(root, state, app_path, app, source_path, source_identity)',
                               '        pass  # producer checks detached'),
                source.replace('        app_path, app = compiled_app(root, staging=Path(stage))',
                               "        app_path, app = None, b'unchecked'"),
                source.replace('    cli = tauri_identity(root)', "    cli = {'entry': 'fake', 'version': 'unchecked'}"),
                source.replace('    zip_inventory(destination, expected)\n', '    # actual final ZIP check removed\n'),
                source.replace('        exact(actual, expected)\n', '        # full ZIP comparison removed\n'),
                source.replace("    require_legal_resources(conf['bundle']['resources'])", "    # mandatory legal registration removed"),
                source.replace('            shutil.copyfile(io_path(regular(source)), io_path(path))', '            pass  # source map no longer copied'),
                source.replace('    build_portable(root, state, app, portable)', '    if False:\n        build_portable(root, state, app, portable)'),
                source.replace("'assemble': assemble, 'contract': contract", "'assemble': contract, 'contract': contract"),
                source.replace(gate, '    False and zip_inventory(destination, expected)\n'),
                source.replace(gate, '    zip_inventory(destination, expected) if False else None\n'),
                source.replace(gate, '    lambda: zip_inventory(destination, expected)\n'),
                source.replace(gate, '    [zip_inventory(destination, expected) for ignored in ()]\n'),
                source.replace(gate, '    0 > 1 > zip_inventory(destination, expected)\n'),
                source.replace(gate, '    return\n' + gate),
                source.replace(gate, '    if destination:\n        return\n' + gate),
                source.replace(gate, '    with contextlib.suppress(Exception):\n    ' + gate),
                source.replace("        for name, source in state['sources'].items():\n",
                               "        if False:\n            for name, source in state['sources'].items():\n                pass\n"),
                source.replace('            shutil.copyfile(io_path(regular(source)), io_path(path))',
                               '            if name:\n                continue\n            shutil.copyfile(io_path(regular(source)), io_path(path))'),
                source.replace("for name, source in state['sources'].items():",
                               "for source, name in state['sources'].items():"),
                source.replace("}[args.action](ROOT)", "}['contract'](ROOT)"),
                source.replace("}[args.action](ROOT)", "}[args.action](None)"),
                source.replace("'prepare': prepare, 'assemble': assemble, 'contract': contract}",
                               "'prepare': prepare, 'assemble': assemble, 'contract': contract, **extra}"),
                source.replace("'prepare': prepare, 'assemble': assemble, 'contract': contract}",
                               "'prepare': prepare, 'assemble': assemble, 'contract': contract, 'prepare': prepare}"),
                source.replace("result = {'prepare': prepare, 'assemble': assemble, 'contract': contract}[args.action](ROOT)",
                               "unused = {'prepare': prepare, 'assemble': assemble, 'contract': contract}\n    result = contract(ROOT)"),
                source.replace("result = {'prepare': prepare, 'assemble': assemble, 'contract': contract}[args.action](ROOT)",
                               "result = False and {'prepare': prepare, 'assemble': assemble, 'contract': contract}[args.action](ROOT)"),
            ]
            for changed in mutations:
                self.assertNotEqual(changed, source)
                with self.assertRaises(ValueError): w.contract(root, source_text=changed, workflow_text=workflow)
            for changed in [workflow.replace('python scripts/windows-package-integrity.py assemble', 'python scripts/windows-package-integrity.py contract'),
                            workflow.replace("if: runner.os == 'Windows'\n        shell: pwsh\n        run: |\n          python scripts/windows-package-integrity.py prepare", "if: false\n        shell: pwsh\n        run: |\n          python scripts/windows-package-integrity.py prepare"),
                            workflow.replace("if ($LASTEXITCODE -ne 0) { throw 'complete Windows carrier integrity failed' }", "Write-Host 'ignored'"),
                            workflow.replace("if: runner.os == 'Windows'\n        shell: pwsh\n        run: |\n          $ErrorActionPreference = 'Stop'\n          python scripts/windows-package-integrity.py assemble", "if: inputs.native_validation == true\n        shell: pwsh\n        run: |\n          $ErrorActionPreference = 'Stop'\n          python scripts/windows-package-integrity.py assemble")]:
                self.assertNotEqual(changed, workflow)
                with self.assertRaises(ValueError): w.contract(root, source_text=source, workflow_text=changed)

    def test_three_legal_members_cannot_disappear_from_config_or_actual_zip(self):
        for name in ['LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md']:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); state, app = self.fixture(root)
                expected = w.carrier_inventory(state, app, 'zip')
                archive = self.zip(root, expected, state, app, lambda payload: payload.pop(name))
                with self.assertRaises(ValueError): w.zip_inventory(archive, expected)
                conf = root / 'src-tauri/tauri.windows.conf.json'; value = json.loads(conf.read_text())
                value['bundle']['resources'].remove('../' + name); conf.write_text(json.dumps(value))
                with self.assertRaises(ValueError): w.source_inventory(root)
                with self.assertRaises(ValueError): w.contract(root, workflow_text=(ROOT / '.github/workflows/package.yml').read_text(encoding='utf-8'))

    def test_streamed_unknown_sizes_are_valid_but_literal_member_bytes_are_exact(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); archive = root / 'inert.exe'; archive.write_bytes(b'inert carrier')
            listing = self.listing([('polaris.exe', None), ('_up_/resources/a[1].txt', None)])
            payloads = {'polaris.exe': b'App', '_up_/resources/a[1].txt': b'legal'}
            budget = {'bytes': 0}; observed_timeouts = []; original = w.stream_child
            def stream(*args, **kwargs):
                observed_timeouts.append(kwargs['timeout']); return original(*args, **kwargs)
            with self.inert_reader(listing, payloads) as (children, commands), patch.object(w, 'stream_child', side_effect=stream):
                actual, _ = w.unpack_nsis('inert-reader', archive, root / 'new', {'polaris.exe': 3, '_up_/resources/a[1].txt': 5}, budget=budget, timeout=5)
            self.assertEqual(actual, {name: w.byte_facts(data) for name, data in payloads.items()})
            self.assertEqual(budget['bytes'], 8)
            self.assertEqual([argv[-1] for argv in commands if argv[1] == 'x'], list(payloads))
            self.assertTrue(all('-so' in argv and '-spd' in argv and '--' in argv for argv in commands if argv[1] == 'x'))
            self.assertTrue(all(a > b > 0 for a, b in zip(observed_timeouts, observed_timeouts[1:])), 'listing and all member streams must share one deadline')

    def test_unknown_size_member_and_shared_aggregate_overflow_stop_before_surplus_write(self):
        for category in ['member', 'aggregate', 'reported-size']:
            with self.subTest(category=category), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); archive = root / 'inert.exe'; archive.write_bytes(b'inert')
                size = 1 if category == 'reported-size' else None
                budget = {'bytes': w.MAX_TOTAL - 2 if category == 'aggregate' else 0}
                with self.inert_reader(self.listing([('polaris.exe', size)]), {'polaris.exe': b'ABCD'}) as (children, _):
                    with self.assertRaises(ValueError):
                        w.unpack_nsis('inert-reader', archive, root / 'owned', {'polaris.exe': 3}, budget=budget)
                self.assertFalse((root / 'owned').exists())
                self.assertLessEqual(budget['bytes'], w.MAX_TOTAL)

    def test_unknown_size_outer_uninstaller_cannot_exceed_real_64MiB_cap(self):
        self.assertEqual(w.MAX_UNINSTALLER, 64 * 1024 ** 2)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); archive = root / 'inert.exe'; archive.write_bytes(b'inert')
            with self.inert_reader(self.listing([('uninstall.exe', None)]), {'uninstall.exe': w.MAX_UNINSTALLER + 1}) as (children, _):
                with self.assertRaisesRegex(ValueError, 'stdout byte limit'):
                    w.unpack_nsis('inert-reader', archive, root / 'owned', {'uninstall.exe': w.MAX_UNINSTALLER})
            self.assertFalse((root / 'owned').exists())

    def test_actual_listing_and_stderr_bytes_are_bounded_and_children_join(self):
        for kind in ['listing', 'stderr']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); archive = root / 'inert.exe'; archive.write_bytes(b'inert')
                listing = w.MAX_LISTING + 1 if kind == 'listing' else self.listing([('polaris.exe', None)])
                with self.inert_reader(listing, {'polaris.exe': b'App'}, stderr_bytes=w.MAX_STDERR + 1 if kind == 'stderr' else 0) as (children, _):
                    with self.assertRaisesRegex(ValueError, 'byte limit'):
                        w.unpack_nsis('inert-reader', archive, root / 'owned', {'polaris.exe': 3})
                self.assertFalse((root / 'owned').exists())

    def test_dual_stream_drain_nonzero_exit_and_timeout_are_real_owned_child_cases(self):
        received = bytearray()
        program = "import sys\nfor _ in range(24):\n sys.stdout.buffer.write(b'a'*4096);sys.stdout.buffer.flush();sys.stderr.buffer.write(b'b'*4096);sys.stderr.buffer.flush()"
        observed = w.stream_child([sys.executable, '-u', '-c', program], received.extend, 24 * 4096, timeout=5)
        self.assertEqual(bytes(received), b'a' * (24 * 4096)); self.assertEqual(observed['stderrBytes'], 24 * 4096)
        for program, timeout, message in [("import sys;sys.stderr.write('failed fixture');sys.exit(3)", 5, 'owned child failed'),
                                           ('import time;time.sleep(5)', .15, 'deadline')]:
            with self.subTest(program=program):
                popen = subprocess.Popen; children = []
                def launch(*args, **kwargs):
                    child = popen(*args, **kwargs); children.append(child); return child
                started = time.monotonic()
                with patch.object(w.subprocess, 'Popen', side_effect=launch):
                    with self.assertRaisesRegex(ValueError, message):
                        w.stream_child([sys.executable, '-u', '-c', program], lambda block: None, 1024, timeout=timeout)
                self.assertTrue(all(child.poll() is not None for child in children))
                self.assertLess(time.monotonic() - started, 3)


@unittest.skipUnless(os.name == 'nt', 'native Windows filesystem required')
class WindowsLongPaths(unittest.TestCase):
    @staticmethod
    def native(path):
        # Independent fixture I/O, not the producer function under test.
        name = os.path.abspath(path)
        if name.startswith('\\\\?\\'): return Path(name)
        return Path('\\\\?\\UNC\\' + name[2:] if name.startswith('\\\\') else '\\\\?\\' + name)

    @contextmanager
    def deep_directory(self):
        with tempfile.TemporaryDirectory(prefix='windows-package-longpath-', dir=self.native(tempfile.gettempdir())) as tmp:
            name = str(tmp)
            root = Path('\\\\' + name[8:] if name.startswith('\\\\?\\UNC\\') else name[4:])
            while len(str(root).encode('utf-16-le')) // 2 <= 280:
                root /= 'long-resource-许可-' + 'x' * 24
            native = self.native(root); native.mkdir(parents=True)
            yield root, native

    def fixture(self, native):
        # Establish the reference inventory at ordinary short paths, then copy
        # actual inert files into an independently created long native tree.
        with tempfile.TemporaryDirectory() as tmp:
            state, app = Integrity().fixture(Path(tmp))
            shutil.copytree(tmp, native, dirs_exist_ok=True)
            return state['resources'], app

    def test_native_long_file_recursive_tree_and_relative_names(self):
        with self.deep_directory() as (root, native):
            leaf = native / 'nested/许可证/LICENSE'; leaf.parent.mkdir(parents=True)
            leaf.write_bytes(b'actual native long-path license')
            expected = w.byte_facts(leaf.read_bytes())
            logical = root / 'nested/许可证/LICENSE'
            self.assertGreater(len(str(logical).encode('utf-16-le')) // 2, 260)
            self.assertEqual(w.regular(logical), logical)
            self.assertEqual(w.facts(logical), expected)
            self.assertEqual(w.tree(root), {'nested/许可证/LICENSE': expected})

    def test_native_long_inventory_portable_and_cargo_known_pair(self):
        with self.deep_directory() as (root, native):
            expected, app = self.fixture(native)
            resources, sources = w.source_inventory(root)
            self.assertEqual(resources, expected)
            self.assertTrue(all(Path(source).is_relative_to(root) and not source.startswith('\\\\?\\')
                                for source in sources.values()), 'logical source maps must stay unprefixed')
            state = {'resources': resources, 'sources': sources, 'owner': str(root)}
            archive = root / 'portable.zip'; w.build_portable(root, state, app, archive)
            self.assertEqual(w.zip_inventory(archive, w.carrier_inventory(state, app, 'zip')),
                             w.carrier_inventory(state, app, 'zip'))
            binary = native / 'target/release/polaris.exe'; binary.parent.mkdir(parents=True); binary.write_bytes(app)
            alias = binary.parent / 'deps/polaris.exe'; alias.parent.mkdir(); os.link(binary, alias)
            before = w.cargo_app_identity(root)[1]
            stage = native / 'private'; stage.mkdir()
            staged, captured = w.compiled_app(root, staging=root / 'private')
            self.assertEqual(staged, root / 'private/polaris.exe')
            self.assertEqual(self.native(staged).read_bytes(), captured)
            self.assertEqual(captured, app)
            self.assertEqual(self.native(staged).stat().st_nlink, 1)
            self.assertEqual(w.cargo_app_identity(root)[1], before)
            self.assertTrue(alias.samefile(binary))
            with self.assertRaises(ValueError): w.regular(root / 'target/release/polaris.exe')

    def test_native_long_prepare_load_preserve_frozen_logical_paths(self):
        with self.deep_directory() as (root, native):
            expected, _ = self.fixture(native)
            _, sources = w.source_inventory(root)
            (native / 'src-tauri/tauri.conf.json').write_text('{"version":"1.0.0"}')
            for name in ['target/release/bundle/nsis', 'dist-win']:
                path = native / name; path.mkdir(parents=True); (path / 'old.exe').write_bytes(b'old')
            with patch.dict(os.environ, {'GITHUB_ENV': str(root / 'env')}), \
                 patch.object(w, 'find_sevenzip', return_value=('7z', '25.01')), \
                 patch.object(w, 'verified_sources', return_value=(expected, sources)), \
                 patch.object(w.subprocess, 'check_output', return_value='a' * 40 + '\n'):
                w.prepare(root)
                owner = root / 'target/release' / next((native / 'target/release').glob('.polaris-package-*')).name
                state_path = owner / 'state.json'
                self.assertIn(str(state_path), (native / 'env').read_text())
                with patch.dict(os.environ, {'POLARIS_WINDOWS_PACKAGE_STATE': str(state_path)}):
                    frozen = w.load_state(root)
                    self.assertEqual(frozen['root'], str(root)); self.assertEqual(frozen['owner'], str(owner))
                    self.assertEqual(frozen['resources'], expected)
                    with patch.object(w, 'verified_sources', return_value=({}, {})), self.assertRaisesRegex(ValueError, 'source resources changed'):
                        w.load_state(root)
                self.assertEqual(self.native(owner / 'previous-nsis/old.exe').read_bytes(), b'old')
                self.assertEqual(self.native(owner / 'previous-dist-win/old.exe').read_bytes(), b'old')

    def test_native_long_inert_nsis_extraction_output(self):
        with self.deep_directory() as (root, native):
            (native / 'inert.exe').write_bytes(b'inert archive')
            inert = Integrity(); payload = {'nested/LICENSE': b'long inert member'}
            with inert.inert_reader(inert.listing([('nested/LICENSE', 17)]), payload):
                actual, _ = w.unpack_nsis('inert-reader', root / 'inert.exe', root / 'owned', {'nested/LICENSE': 17})
            self.assertEqual(actual, {'nested/LICENSE': w.byte_facts(payload['nested/LICENSE'])})
            self.assertEqual((native / 'owned/nested/LICENSE').read_bytes(), payload['nested/LICENSE'])

    def test_native_long_hardlinks_junction_ancestors_and_name_aliases_rejected(self):
        from _winapi import CreateJunction
        with self.deep_directory() as (root, native):
            leaf = native / 'LICENSE'; leaf.write_bytes(b'license'); alias = native / 'hardlink'
            os.link(leaf, alias)
            with self.assertRaises(ValueError): w.facts(root / 'LICENSE')
            with self.assertRaises(ValueError): w.tree(root)
            alias.unlink()
            # CreateJunction's target probe uses ordinary Win32 paths. Keep
            # that owned target short; the junction and tested leaf stay long.
            with tempfile.TemporaryDirectory(prefix='windows-junction-target-') as tmp:
                target = Path(tmp); (target / 'file').write_bytes(b'data')
                junction = native / 'junction'; CreateJunction(str(target), str(junction))
                try:
                    self.assertEqual((junction / 'file').read_bytes(), b'data', 'fixture junction must resolve before testing refusal')
                    with self.assertRaises(ValueError): w.tree(root)
                    with self.assertRaises(ValueError): w.tree(root / 'junction')
                    with self.assertRaises(ValueError): w.facts(root / 'junction/file')
                finally: junction.rmdir()
            for name in ['ambiguous.', 'trailing ', 'NUL.txt']:
                path = native / name; path.write_bytes(b'unregistered alias')
                try:
                    with self.assertRaises(ValueError): w.tree(root)
                finally: path.unlink()

    def test_native_long_budgets_and_hash_change_guards(self):
        with self.deep_directory() as (root, native):
            leaf = native / 'a'; leaf.write_bytes(b'1234'); (native / 'b').write_bytes(b'5678')
            with patch.object(w, 'MAX_FILE', 3), self.assertRaises(ValueError): w.facts(root / 'a')
            with patch.object(w, 'MAX_TOTAL', 7), self.assertRaises(ValueError): w.tree(root)
            with patch.object(w, 'MAX_MEMBERS', 1), self.assertRaises(ValueError): w.tree(root)
            original = w.hashlib.sha256
            class ChangingDigest:
                def __init__(self): self.digest = original(); self.changed = False
                def update(self, block):
                    self.digest.update(block)
                    if not self.changed:
                        self.changed = True; leaf.write_bytes(b'changed file bytes')
                def hexdigest(self): return self.digest.hexdigest()
            with patch.object(w.hashlib, 'sha256', ChangingDigest), self.assertRaisesRegex(ValueError, 'member changed while hashing'):
                w.facts(root / 'a')

    def test_native_only_drive_and_unc_filesystem_namespaces(self):
        self.assertEqual(str(w.io_path(Path(r'\\server\share\directory\file'))), r'\\?\UNC\server\share\directory\file')
        for name in [r'\\.\NUL', r'\\?\GLOBALROOT\Device\HarddiskVolume1\file', r'\\?\Volume{00000000-0000-0000-0000-000000000000}\file']:
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'unsupported Windows filesystem namespace'):
                w.io_path(name)


if __name__ == '__main__': unittest.main()
