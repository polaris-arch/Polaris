"""Pure format, provenance, cache and rollback controls; no compiler/device runs."""
from copy import deepcopy
import ast
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

from fork_input import ROOT, canonical, load, sha
from artifacts import component_output_identity, framework_entry_targets, module, module_stream, objc_hot_contract, objc_validation_getter_contract, tree_inventory, zip_entries
from budget import capacity_limit, native_capacity, native_environment, reservation
from ci_android import emit_outputs, prepare_source, prepare
from producer import PRODUCER_FILES, compiler_fact, equivalent_binding, executable_driver_identity
from transaction import install, recover
import artifacts
from producer import write_json


class FormalOutputs(unittest.TestCase):
    def setUp(self):
        self.value = load()
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.source = Path(temporary.name).resolve()

    def test_native_capacity_caps_current_runner_by_load_affinity_quota_and_memory(self):
        facts = {'logicalCPUs': 16, 'affinityCPUs': 8, 'quotaCPUs': 4.5, 'load1': 1.1,
                 'availableMemoryBytes': 9 * 1024 ** 3,
                 'memoryHeadroomBytes': 1024 ** 3, 'memoryPerJobBytes': 2 * 1024 ** 3}
        observed = capacity_limit(facts)
        self.assertEqual(observed['effectiveCPUs'], 4)
        self.assertEqual(observed['loadJobCap'], 2)
        self.assertEqual(observed['memoryJobCap'], 4)
        self.assertEqual(observed['jobCap'], 2)
        original = {'CARGO_BUILD_JOBS': '1', 'UNCHANGED': 'fixture'}
        child, applied = native_environment(original, observed)
        self.assertEqual(child, dict(original, GOMAXPROCS='2'))
        self.assertEqual(original, {'CARGO_BUILD_JOBS': '1', 'UNCHANGED': 'fixture'})
        self.assertEqual(applied['appliedEnvironment'], {'CARGO_BUILD_JOBS': 1, 'GOMAXPROCS': 2})
        self.assertNotIn('appliedEnvironment', observed)

    def test_native_capacity_unknowns_and_low_memory_guide_concurrency_without_refusing_builds(self):
        facts = {'logicalCPUs': None, 'affinityCPUs': None, 'quotaCPUs': None, 'load1': None,
                 'availableMemoryBytes': None, 'memoryHeadroomBytes': 1024 ** 3,
                 'memoryPerJobBytes': 2 * 1024 ** 3}
        observed = capacity_limit(facts)
        self.assertEqual(observed['jobCap'], 1)
        self.assertEqual(observed['unknown'], ['cpu', 'load', 'memory'])
        self.assertIsNone(observed['estimatedMemoryJobCap'])
        self.assertEqual(observed['memoryEstimateRisk'], 'unknown-memory')
        for memory in (0, 1024 ** 3, 3 * 1024 ** 3 - 1):
            with self.subTest(memory=memory):
                facts.update(logicalCPUs=8, load1=0, availableMemoryBytes=memory)
                limited = capacity_limit(facts)
                self.assertEqual(limited['estimatedMemoryJobCap'], 0)
                self.assertEqual(limited['memoryJobCap'], 1)
                self.assertEqual(limited['memoryEstimateRisk'], 'below-unmeasured-single-job-estimate')
                child, applied = native_environment({}, limited)
                self.assertEqual(child, {'CARGO_BUILD_JOBS': '1', 'GOMAXPROCS': '1'})
                self.assertIsNone(applied['peakRSSBytes'])
        child, applied = native_environment({}, observed)
        self.assertEqual(child, {'CARGO_BUILD_JOBS': '1', 'GOMAXPROCS': '1'})
        self.assertEqual(applied['unknown'], ['cpu', 'load', 'memory'])

    def test_native_capacity_retains_strict_explicit_caps_and_refuses_only_invalid_limits(self):
        facts = {'logicalCPUs': 8, 'affinityCPUs': None, 'quotaCPUs': None, 'load1': 0,
                 'availableMemoryBytes': 17 * 1024 ** 3, 'memoryHeadroomBytes': 1024 ** 3,
                 'memoryPerJobBytes': 2 * 1024 ** 3}
        observed = capacity_limit(facts)
        original = {'CARGO_BUILD_JOBS': '2', 'GOMAXPROCS': '1'}
        child, _ = native_environment(original, observed)
        self.assertEqual(child, original)
        for key in ('CARGO_BUILD_JOBS', 'GOMAXPROCS'):
            for value in ('0', '-1', 'auto', '١', '', True, 2):
                with self.subTest(key=key, value=value), self.assertRaisesRegex(RuntimeError, 'positive integer'):
                    native_environment({key: value}, observed)
        for cap in (0, -1, True, '1'):
            with self.subTest(cap=cap), self.assertRaisesRegex(RuntimeError, 'observation must be'):
                native_environment({}, dict(observed, jobCap=cap))

    def test_native_capacity_reads_parent_cgroup_limits_without_exporting_environment(self):
        text = {'/proc/self/cgroup': '0::/runner/job\n',
                '/proc/meminfo': 'MemAvailable: 16777216 kB\n',
                '/sys/fs/cgroup/runner/job/cpu.max': 'max 100000\n',
                '/sys/fs/cgroup/runner/cpu.max': '200000 100000\n',
                '/sys/fs/cgroup/cpu.max': 'max 100000\n',
                '/sys/fs/cgroup/runner/memory.max': str(7 * 1024 ** 3),
                '/sys/fs/cgroup/runner/memory.current': str(2 * 1024 ** 3)}
        with patch('budget.sys.platform', 'linux'), patch('budget.os.cpu_count', return_value=16), \
                patch('budget.os.sched_getaffinity', return_value=set(range(8)), create=True), \
                patch('budget.os.getloadavg', return_value=(0, 0, 0)), \
                patch('budget._capacity_text', side_effect=lambda path: text.get(str(path))), \
                patch.dict(os.environ, {'DO_NOT_EXPORT_SECRET': 'fixture'}):
            observed = native_capacity()
        self.assertEqual(observed['jobCap'], 2)
        self.assertEqual(observed['quotaCPUs'], 2)
        self.assertEqual(observed['availableMemoryBytes'], 5 * 1024 ** 3)
        self.assertEqual(observed['memorySource'], 'cgroup-v2-headroom')
        self.assertEqual(observed['osMemoryObservationBytes'], 16 * 1024 ** 3)
        self.assertEqual(observed['cgroupMemoryConstraints'][0]['limitBytes'], 7 * 1024 ** 3)
        self.assertEqual(observed['cgroupMemoryConstraints'][0]['usageBytes'], 2 * 1024 ** 3)
        self.assertNotIn('DO_NOT_EXPORT_SECRET', json.dumps(observed))

    def test_native_capacity_darwin_uses_actual_vm_pages_and_bounded_command(self):
        result = type('Result', (), {'stdout': b'Mach Virtual Memory Statistics: (page size of 16384 bytes)\n'
                    b'Pages free: 300000.\nPages inactive: 600000.\nPages speculative: 0.\n'})()
        with patch('budget.sys.platform', 'darwin'), patch('budget.os.cpu_count', return_value=8), \
                patch('budget.os.sched_getaffinity', side_effect=AttributeError, create=True), \
                patch('budget.os.getloadavg', return_value=(1, 0, 0)), \
                patch('budget.subprocess.run', return_value=result) as command:
            observed = native_capacity()
        self.assertEqual(observed['availableMemoryBytes'], 900000 * 16384)
        self.assertEqual(observed['osMemoryObservationKind'],
                         'reclaimable-page-subset-not-complete-available-memory')
        self.assertEqual(observed['jobCap'], 6)
        self.assertTrue(observed['quotaObserved'])
        self.assertIsNone(observed['quotaCPUs'])
        self.assertEqual(observed['unknown'], [])
        self.assertEqual(command.call_args.args[0], ['/usr/bin/vm_stat'])
        self.assertEqual(command.call_args.kwargs['timeout'], 2)
        self.assertEqual(command.call_args.kwargs['env'], {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C'})

    def test_native_capacity_legacy_cgroup_limits_and_missing_quota_are_conservative(self):
        text = {'/proc/self/cgroup': '2:cpu,cpuacct:/job\n3:memory:/job\n',
                '/proc/meminfo': 'MemAvailable: 16777216 kB\n',
                '/sys/fs/cgroup/cpu,cpuacct/job/cpu.cfs_quota_us': '350000',
                '/sys/fs/cgroup/cpu,cpuacct/job/cpu.cfs_period_us': '100000',
                '/sys/fs/cgroup/memory/job/memory.limit_in_bytes': str(6 * 1024 ** 3),
                '/sys/fs/cgroup/memory/job/memory.usage_in_bytes': str(1024 ** 3)}
        with patch('budget.sys.platform', 'linux'), patch('budget.os.cpu_count', return_value=16), \
                patch('budget.os.sched_getaffinity', return_value=set(range(8)), create=True), \
                patch('budget.os.getloadavg', return_value=(0, 0, 0)), \
                patch('budget._capacity_text', side_effect=lambda path: text.get(str(path))):
            observed = native_capacity()
            self.assertEqual(observed['jobCap'], 2)
            self.assertEqual(observed['cgroupVersion'], 1)
            self.assertEqual(observed['quotaCPUs'], 3.5)
            self.assertEqual(observed['memorySource'], 'cgroup-v1-headroom')
            text.clear()
            unknown = native_capacity()
        self.assertEqual(unknown['jobCap'], 1)
        self.assertIn('cpu-quota', unknown['unknown'])
        self.assertIn('memory', unknown['unknown'])
        self.assertIsNone(unknown['quotaCPUs'])

    def test_native_capacity_low_darwin_subset_and_unavailable_sampling_remain_serial(self):
        result = type('Result', (), {'stdout': b'Mach Virtual Memory Statistics: (page size of 16384 bytes)\n'
                    b'Pages free: 1000.\nPages inactive: 2000.\nPages speculative: 0.\n'})()
        with patch('budget.sys.platform', 'darwin'), patch('budget.os.cpu_count', return_value=8), \
                patch('budget.os.sched_getaffinity', side_effect=AttributeError, create=True), \
                patch('budget.os.getloadavg', return_value=(0, 0, 0)), \
                patch('budget.subprocess.run', return_value=result) as command:
            sample = native_capacity()
            self.assertEqual(sample['osMemoryObservationBytes'], 3000 * 16384)
            self.assertEqual(sample['estimatedMemoryJobCap'], 0)
            self.assertEqual(sample['jobCap'], 1)
            child, applied = native_environment({}, sample)
            self.assertEqual(child, {'CARGO_BUILD_JOBS': '1', 'GOMAXPROCS': '1'})
            self.assertEqual(applied['memorySource'], 'vm-stat-free-inactive-speculative')
            command.side_effect = OSError('controlled unavailable sampler')
            missing = native_capacity()
        self.assertIsNone(missing['availableMemoryBytes'])
        self.assertIsNone(missing['osMemoryObservationBytes'])
        self.assertEqual(missing['memoryEstimateRisk'], 'unknown-memory')
        self.assertEqual(missing['jobCap'], 1)
        self.assertNotIn('cpu-quota', missing['unknown'])

    def test_native_prepare_and_app_calls_flush_samples_before_invalid_env_without_compiler_calls(self):
        # Execute only the actual capacity call-site statements extracted from
        # these callers. All runner sampling/output/compiler hooks are controlled;
        # this is an error-state control, not a native or full prepare execution.
        class FlushedOutput(io.StringIO):
            def __init__(self):
                super().__init__()
                self.flush_count = 0

            def flush(self):
                self.flush_count += 1
                super().flush()

        cases = [('ci_android.py', 'prepare', 'android-tool-prepare-capacity-sample'),
                 ('ci_apple.py', 'prepare', 'apple-tool-prepare-capacity-sample'),
                 ('build_apple_app.py', 'main', 'apple-app-build-capacity-sample')]
        for filename, function_name, phase in cases:
            with self.subTest(caller=filename):
                path = ROOT / 'scripts/mobile-core' / filename
                tree = ast.parse(path.read_text(), filename=str(path))
                function = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == function_name)
                blocks = [node.body for node in ast.walk(function) if hasattr(node, 'body') and isinstance(node.body, list)]
                selected = []
                for statements in blocks:
                    for position, node in enumerate(statements):
                        if (isinstance(node, ast.Assign) and isinstance(node.value, ast.Call)
                                and isinstance(node.value.func, ast.Name) and node.value.func.id == 'native_capacity'):
                            selected.append(statements[position:position + 5])
                self.assertEqual(len(selected), 1)
                statements = selected[0]
                self.assertEqual(len(statements), 5)
                self.assertIsInstance(statements[-1], ast.Expr)
                self.assertIn(statements[-1].value.func.id, ('run', 'timed_phase'))
                stream = FlushedOutput()
                sample = {'jobCap': 1, 'availableMemoryBytes': 0,
                          'scope': 'controlled original sample; not runner observation'}
                sampler, compiler, phase_runner = Mock(return_value=sample), Mock(), Mock()

                def apply(env, observation):
                    self.assertIs(observation, sample)
                    self.assertEqual(stream.flush_count, 1)
                    self.assertEqual(json.loads(stream.getvalue()), {'phase': phase, 'capacity': sample})
                    return native_environment(env, observation)

                namespace = {'json': json, 'env': {'GOMAXPROCS': 'auto'},
                             'native_capacity': sampler, 'native_environment': apply,
                             'run': compiler, 'timed_phase': phase_runner}
                code = compile(ast.Module(body=statements, type_ignores=[]), str(path), 'exec')
                with patch('sys.stdout', stream), self.assertRaisesRegex(RuntimeError, 'positive integer'):
                    exec(code, namespace)
                sampler.assert_called_once_with()
                compiler.assert_not_called()
                phase_runner.assert_not_called()
                self.assertEqual(stream.flush_count, 1)

    def test_large_apple_receipts_round_trip_original_inventory_and_fingerprint(self):
        # Model the real producer/consumer boundary, not native acceptance.
        # The full JSON is larger than the former 16MiB reader envelope.
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'receipt.json'
            for schema in ('polaris-mobile-component-v1', 'polaris-mobile-apple-link-input-v1'):
                value = {'schema': schema, 'platform': 'apple',
                         'originalInventoryObservation': ['x' * 800 for _ in range(22000)],
                         'AppAccepted': False, 'devicePassed': False}
                value['fingerprint'] = sha(canonical(value))
                write_json(path, value)
                self.assertGreater(path.stat().st_size, artifacts.MAX_RECEIPT)
                self.assertEqual(artifacts.read_receipt(path), value)
                self.assertEqual(value['fingerprint'], sha(canonical({k: v for k, v in value.items() if k != 'fingerprint'})))

    def test_receipt_budgets_refuse_unknown_android_oversize_and_preserve_prior_bytes(self):
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(artifacts, 'MAX_RECEIPT', 256), patch.object(artifacts, 'MAX_APPLE_RECEIPT', 1024):
            path = Path(temporary) / 'receipt.json'
            original = b'{"schema":"small-original"}\n'; path.write_bytes(original)
            for value in [{'schema': 'unknown'}, {'schema': 'polaris-mobile-component-v1', 'platform': 'android'},
                          {'schema': 'polaris-mobile-component-v1', 'platform': 'apple', 'payload': 'x' * 1024}]:
                value.setdefault('payload', 'x' * 256)
                with self.assertRaisesRegex(RuntimeError, 'before installation'):
                    write_json(path, value)
                self.assertEqual(path.read_bytes(), original)
                self.assertFalse(path.with_name('receipt.json.tmp').exists())
                path.write_text(json.dumps(value))
                with self.assertRaises(RuntimeError):
                    artifacts.read_receipt(path)
                path.write_bytes(original)
            for invalid in [b'[]', b'{"a":1,"a":2}', b'{"a":NaN}', b'']:
                path.write_bytes(invalid)
                with self.assertRaises(RuntimeError):
                    artifacts.read_receipt(path)
            path.write_bytes(original)
            link = path.with_name('link.json'); link.symlink_to(path.name)
            with self.assertRaisesRegex(RuntimeError, 'bounded regular'):
                artifacts.read_receipt(link)

    def test_receipt_size_observation_precedes_rejection_and_preserves_actual_lstat_size(self):
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(artifacts, 'MAX_APPLE_RECEIPT', 128), patch('sys.stderr', new_callable=io.StringIO) as log:
            path = Path(temporary) / 'component-receipt.json'; path.write_bytes(b'x' * 129)
            with self.assertRaisesRegex(RuntimeError, 'bounded regular'):
                artifacts.read_receipt(path)
            read = json.loads(log.getvalue().splitlines()[0])
            self.assertEqual(read['bytes'], path.stat().st_size)
            self.assertEqual(read['maximumReadBytes'], 128)
            self.assertEqual(read['phase'], 'read')
            log.seek(0); log.truncate()
            with self.assertRaisesRegex(RuntimeError, 'before installation'):
                write_json(path, {'schema': 'polaris-mobile-component-v1', 'platform': 'apple', 'raw': 'x' * 129})
            write = json.loads(log.getvalue().splitlines()[0])
            self.assertEqual(write['maximumBytes'], 128)
            self.assertEqual(write['phase'], 'bounded-serialization-rejected')
            self.assertNotIn('bytes', write, 'rejected bounded stream must not invent a complete size')
            self.assertEqual(path.read_bytes(), b'x' * 129)

    def test_component_capacity_does_not_expand_link_android_or_control_receipt_budgets(self):
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(artifacts, 'MAX_RECEIPT', 256), \
                patch.object(artifacts, 'MAX_APPLE_LINK_RECEIPT', 512), \
                patch.object(artifacts, 'MAX_APPLE_RECEIPT', 1024):
            path = Path(temporary) / 'receipt.json'
            component = {'schema': 'polaris-mobile-component-v1', 'platform': 'apple',
                         'originalNativeInventory': 'x' * 600}
            component['fingerprint'] = sha(canonical(component))
            write_json(path, component)
            self.assertEqual(artifacts.read_receipt(path), component)
            original = path.read_bytes()
            for changed in [dict(component, platform='android'),
                            dict(component, schema='polaris-mobile-apple-link-input-v1'),
                            dict(component, schema='unknown-control'),
                            dict(component, originalNativeInventory='x' * 1024)]:
                with self.assertRaisesRegex(RuntimeError, 'before installation'):
                    write_json(path, changed)
                self.assertEqual(path.read_bytes(), original)
                path.write_text(json.dumps(changed))
                with self.assertRaises(RuntimeError):
                    artifacts.read_receipt(path)
                path.write_bytes(original)

    def test_streaming_fingerprint_and_pretty_bytes_keep_canonical_contract(self):
        value = {'schema': 'small-control', 'utf8': '中文', 'number': 1.25, 'nested': [None, True, {'b': 2, 'a': 1}]}
        self.assertEqual(artifacts.receipt_digest(value), sha(canonical(value)))
        self.assertEqual(b''.join(artifacts.receipt_chunks(value, pretty=True)),
                         (json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + '\n').encode())
        self.assertEqual(artifacts.receipt_field_bytes(value), {key: len(canonical(child)) for key, child in value.items()})
        for value in [{'raw': 'x' * (artifacts.MAX_JSON_STRING + 1)}, {'raw': float('inf')}, {'raw': 1 << 257}]:
            with self.assertRaises(RuntimeError): artifacts.receipt_digest(value)
        cycle = {}; cycle['self'] = cycle
        with self.assertRaisesRegex(RuntimeError, 'cyclic'): artifacts.receipt_digest(cycle)
        value = {}; node = value
        for _ in range(66): node['child'] = {}; node = node['child']
        with self.assertRaisesRegex(RuntimeError, 'depth'): artifacts.receipt_digest(value)
        with patch.object(artifacts, 'MAX_JSON_NODES', 4), self.assertRaisesRegex(RuntimeError, 'node'):
            artifacts.receipt_digest({'many': [1, 2, 3]})

    def test_reader_checks_lexical_depth_before_building_a_dom(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'receipt.json'; path.write_bytes(b'[' * 65 + b'0' + b']' * 65)
            with patch('artifacts.decode') as decode, self.assertRaisesRegex(RuntimeError, 'depth'):
                artifacts.read_receipt(path)
            decode.assert_not_called()

    def hot_objc_header(self):
        # Source-derived expectations from fresh libbox Go signatures plus
        # sagernet/gomobile v0.1.13 genobjc.go/types.go, not a fresh generated
        # framework observation or evidence that Swift compilation succeeded.
        return '''@class LibboxCommandServer;
@protocol LibboxRuntimePersistenceHandler;
@class LibboxRuntimePersistenceHandler;
@protocol LibboxRuntimePersistenceHandler <NSObject>
- (BOOL)persistRuntimeConfiguration:(NSString* _Nullable)incarnation revision:(int64_t)revision effectiveDigest:(NSString* _Nullable)effectiveDigest configContent:(NSString* _Nullable)configContent error:(NSError* _Nullable* _Nullable)error;
- (NSString* _Nonnull)runtimePersistenceMode;
@end
@interface LibboxCommandServer : NSObject <goSeqRefInterface> {
}
@property(strong, readonly) _Nonnull id _ref;
- (NSString* _Nonnull)runtimeCapabilities:(NSError* _Nullable* _Nullable)error;
- (BOOL)startOrReloadServiceWithRetryableRuntimePersistence:(NSString* _Nullable)configContent options:(LibboxOverrideOptions* _Nullable)options handler:(id<LibboxRuntimePersistenceHandler> _Nullable)handler error:(NSError* _Nullable* _Nullable)error;
@end
@interface LibboxOverrideOptions : NSObject <goSeqRefInterface> {
}
@property(strong, readonly) _Nonnull id _ref;
- (nonnull instancetype)initWithRef:(_Nonnull id)ref;
- (nonnull instancetype)init;
@end
@class LibboxRuntimePersistenceHandler;
@interface LibboxRuntimePersistenceHandler : NSObject <goSeqRefInterface, LibboxRuntimePersistenceHandler> {
}
@property(strong, readonly) _Nonnull id _ref;
- (nonnull instancetype)initWithRef:(_Nonnull id)ref;
- (BOOL)persistRuntimeConfiguration:(NSString* _Nullable)incarnation revision:(int64_t)revision effectiveDigest:(NSString* _Nullable)effectiveDigest configContent:(NSString* _Nullable)configContent error:(NSError* _Nullable* _Nullable)error;
- (NSString* _Nonnull)runtimePersistenceMode;
@end
'''

    def test_objc_hot_contract_requires_exact_owners_and_pinned_generator_types(self):
        header = self.hot_objc_header()
        facts = objc_hot_contract(header)
        self.assertEqual(set(facts), {'@interface LibboxCommandServer',
                         '@protocol LibboxRuntimePersistenceHandler',
                         '@interface LibboxRuntimePersistenceHandler', '@interface LibboxOverrideOptions'})
        self.assertEqual(objc_hot_contract(header.replace('\n', ' ')), facts)
        self.assertEqual(objc_hot_contract(header.replace('*', ' * ').replace(':', ' : ')), facts)
        capability = '- (NSString* _Nonnull)runtimeCapabilities:(NSError* _Nullable* _Nullable)error;'
        retry = next(line for line in header.splitlines() if ')startOrReloadServiceWithRetryable' in line)
        persist = next(line for line in header.splitlines() if ')persistRuntimeConfiguration:' in line)
        mode = '- (NSString* _Nonnull)runtimePersistenceMode;'
        cases = [
            header.replace('LibboxCommandServer :', 'LibboxOtherReceiver :'),
            header.replace(capability, '') + '@interface Other : NSObject\n' + capability + '\n@end',
            header.replace(capability, '// ' + capability),
            header.replace(capability, '/* ' + capability + ' */'),
            header.replace(capability, capability + ' ' + capability),
            header.replace(capability, capability.replace('- ', '+ ')),
            header.replace(capability, capability.replace('NSString* _Nonnull', 'BOOL')),
            header.replace(capability, capability.replace('_Nonnull', '_Nullable')),
            header.replace(capability, '- (BOOL)runtimeCapabilities:(NSString* _Nonnull* _Nullable)ret0_ error:(NSError* _Nullable* _Nullable)error;'),
            header.replace(retry, retry.replace('BOOL', 'void')),
            header.replace(retry, retry.replace('options:', 'overrideOptions:')),
            header.replace(retry, retry.replace('options:(LibboxOverrideOptions* _Nullable)options handler:(id<LibboxRuntimePersistenceHandler> _Nullable)handler',
                                                'handler:(id<LibboxRuntimePersistenceHandler> _Nullable)handler options:(LibboxOverrideOptions* _Nullable)options')),
            header.replace(retry, retry.replace('id<LibboxRuntimePersistenceHandler> _Nullable', 'LibboxRuntimePersistenceHandler* _Nullable')),
            header.replace(persist, persist.replace('revision:(int64_t)', 'revision:(int32_t)'), 1),
            header.replace(persist, persist.replace('effectiveDigest:', 'effectiveDigestWrong:'), 1),
            header.replace(persist, persist.replace('BOOL', 'void'), 1),
            header.replace(mode, mode.replace('_Nonnull', '_Nullable'), 1),
            header.replace('@protocol LibboxRuntimePersistenceHandler <NSObject>', '@protocol LibboxRuntimePersistenceHandler <Other>'),
            header.replace('<goSeqRefInterface, LibboxRuntimePersistenceHandler>', '<goSeqRefInterface>'),
            header.replace('<goSeqRefInterface, LibboxRuntimePersistenceHandler>', '<LibboxRuntimePersistenceHandler>'),
            header.replace('<goSeqRefInterface, LibboxRuntimePersistenceHandler>', '<goSeqRefInterface, Other>'),
            header.replace('- (nonnull instancetype)init;', '- (nullable instancetype)init;'),
            header.replace('@protocol LibboxRuntimePersistenceHandler <NSObject>', '@protocol LibboxRuntimePersistenceHandler <NSObject>\n@optional'),
            header + header,
        ]
        # Each pointer/nullability position and both callback owners matter.
        for declaration in (capability, retry, persist):
            for wrong_error in ('NSError* _Nullable', 'NSError* _Nonnull* _Nullable',
                                'NSError* _Nullable* _Nonnull', 'NSString* _Nullable* _Nullable'):
                cases.append(header.replace(declaration, declaration.replace('NSError* _Nullable* _Nullable', wrong_error), 1))
        for original in ('NSString* _Nullable', 'LibboxOverrideOptions* _Nullable',
                         'id<LibboxRuntimePersistenceHandler> _Nullable'):
            cases.append(header.replace(retry, retry.replace(original, original.replace('_Nullable', '_Nonnull')), 1))
        proxy = header.index('@interface LibboxRuntimePersistenceHandler :')
        for declaration in (persist, mode):
            cases.append(header[:proxy] + header[proxy:].replace(declaration, '', 1))
            cases.append(header[:proxy] + header[proxy:].replace(declaration, declaration.replace('- ', '+ '), 1))
        protocol_start = header.index('@protocol LibboxRuntimePersistenceHandler <')
        protocol_end = header.index('@end', protocol_start) + len('@end')
        cases.append(header[:protocol_start] + header[protocol_end:])
        for wrong in cases:
            with self.subTest(header=wrong[:180]), self.assertRaises(RuntimeError):
                objc_hot_contract(wrong)

    def validation_result_objc_header(self):
        # Declaration spelling/receiver/return type from the preserved original
        # ac6 Libbox.objc.h (129711B, e743b050...), not the former bare-name stub.
        selectors = ['getCleanup', 'getCleanupError', 'getConfigDigest', 'getContractVersion', 'getRequestID',
                     'getTailscaleStoreMembership', 'getTailscaleStoreRetirement', 'getValidation', 'getValidationError']
        return ('@interface LibboxConfigValidationResult : NSObject <goSeqRefInterface> {\n}\n'
                '@property(strong, readonly) _Nonnull id _ref;\n' +
                '\n'.join('- (NSString* _Nonnull)' + name + ';' for name in selectors) + '\n@end\n')

    def test_objc_validation_getters_require_exact_result_receiver_types_and_readonly_selectors(self):
        header = self.validation_result_objc_header()
        facts = objc_validation_getter_contract(header)
        self.assertEqual(facts['receiver'], 'LibboxConfigValidationResult')
        self.assertEqual(len(facts['getters']), 9)
        self.assertEqual(objc_validation_getter_contract(header.replace('\n', ' ')), facts,
                         'valid declaration formatting cannot depend on newlines')
        getter = '- (NSString* _Nonnull)getTailscaleStoreRetirement;'
        membership = '- (NSString* _Nonnull)getTailscaleStoreMembership;'
        for wrong in [
            header.replace(getter, ''), header.replace(membership, ''),
            header.replace(getter, '// ' + getter), header.replace(getter, '/* ' + getter + ' */'),
            header.replace(getter, getter.replace('getTailscaleStoreRetirement', 'tailscaleStoreRetirement')),
            header.replace(getter, getter.replace('NSString* _Nonnull', 'int64_t')),
            header.replace(getter, getter.replace('_Nonnull', '_Nullable')),
            header.replace(getter, getter.replace(';', ': (NSString*)argument;')),
            header.replace(getter, getter.replace('- ', '+ ')),
            header.replace(getter, getter + '\n' + getter),
            header.replace(getter, getter + ' ' + getter),
            header.replace(getter, getter + ' - (void)setTailscaleStoreRetirement:(NSString*)value;'),
            header.replace('LibboxConfigValidationResult', 'LibboxOtherReceiver'),
            header.replace('goSeqRefInterface', 'OtherProtocol'), header + header,
            header.replace('@end', '- (void)setTailscaleStoreRetirement:(NSString*)value;\n@end'),
            header.replace('@end', '@property(strong) NSString* tailscaleStoreMembership;\n@end'),
        ]:
            with self.subTest(header=wrong[:150]), self.assertRaises(RuntimeError):
                objc_validation_getter_contract(wrong)
        wrong_receiver = header.replace(getter, '') + ('@interface LibboxOtherReceiver : NSObject\n' + getter + '\n@end\n')
        with self.assertRaises(RuntimeError):
            objc_validation_getter_contract(wrong_receiver)

    def facts(self, apple=False, simulator=False):
        tags = self.value['apple' if apple else 'android']['tags'][:]
        if apple:
            tags += ['ios', *(['iossimulator'] if simulator else [])]
        facts = {'goVersion': self.value['goVersion'], 'settings': {'GOOS': 'ios' if apple else 'android',
            'GOARCH': 'arm64', 'CGO_ENABLED': '1', '-buildmode': 'c-archive' if apple else 'c-shared',
            '-trimpath': 'true', '-tags': ','.join(tags)}, 'modules': {'example.org/module':
            {'kind': 'dep', 'version': 'v1.2.3', 'sum': 'h1:fixed'}, self.value['goModule']:
            {'kind': 'dep', 'version': 'v0.0.0-00010101000000-000000000000', 'replacement': str(self.source)}}}
        for name, pin in self.value['sourcePolicy']['vendors'].items():
            facts['modules'][name] = {'kind': 'dep', 'version': pin['version'], 'sum': '',
                'replacement': pin['replacementPath'], 'replacementVersion': '(devel)', 'replacementSum': ''}
        return facts

    def test_module_stream_requires_unique_actual_rows(self):
        self.assertEqual(module_stream('{"Path":"a"}\n {"Path":"b"}'), [{'Path': 'a'}, {'Path': 'b'}])
        for bad in ['', '{"Path":"a"}{"Path":"a"}', '[]', '{"Version":"v1"}']:
            with self.subTest(raw=bad), self.assertRaises(RuntimeError):
                module_stream(bad)

    def test_framework_internal_links_and_empty_resources_are_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'Versions/A').mkdir(parents=True)
            (root / 'Versions/A/Libbox').write_bytes(b'static original')
            (root / 'Versions/A/empty').write_bytes(b'')
            (root / 'Versions/Current').symlink_to('A', target_is_directory=True)
            (root / 'Libbox').symlink_to('Versions/Current/Libbox')
            rows = tree_inventory(root)
            self.assertTrue(any(r.get('target') == 'Versions/Current/Libbox' for r in rows))
            (root / 'escape').symlink_to('/etc/passwd')
            with self.assertRaises(RuntimeError):
                tree_inventory(root)

    def test_zip_rejects_duplicates_escape_and_links(self):
        for names, mode in [(['../escape'], None), (['/absolute'], None), (['a', 'a'], None), (['link'], 0o120777)]:
            stream = io.BytesIO()
            with zipfile.ZipFile(stream, 'w') as bundle:
                for name in names:
                    item = zipfile.ZipInfo(name)
                    if mode:
                        item.external_attr = mode << 16
                    bundle.writestr(item, b'fixture')
            with zipfile.ZipFile(io.BytesIO(stream.getvalue())) as bundle, self.assertRaises(RuntimeError):
                zip_entries(bundle)

    def test_cache_reuse_preserves_original_producer_commit_only(self):
        original = {'producer': {'candidate': 'old', 'files': {'code': {'sha256': 'same'}}}, 'tools': {'go': 'fixed'}}
        current = deepcopy(original)
        current['producer']['candidate'] = 'new'
        self.assertTrue(equivalent_binding(original, current))
        current['tools']['go'] = 'different'
        self.assertFalse(equivalent_binding(original, current))
        current = deepcopy(original)
        current['producer']['files']['code']['sha256'] = 'changed'
        self.assertFalse(equivalent_binding(original, current))

        original['platform'] = 'android'
        original['producer']['files'] = {name: {'bytes': 4, 'sha256': '1' * 64} for name in PRODUCER_FILES}
        current = deepcopy(original)
        current['producer']['candidate'] = 'new'
        self.assertTrue(equivalent_binding(original, current))
        inventory = original['producer']['files']
        inventory['scripts/mobile-core/prepare-ndk-linux.py'] = inventory.pop('scripts/mobile-core/prepare-ndk-linux.mjs')
        before = deepcopy(original)
        self.assertFalse(equivalent_binding(original, current))
        self.assertEqual(original, before)

    def test_pair_validation_failure_restores_both_original_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            pairs = []
            for name in ('component', 'receipt'):
                a, b = root / ('stage-' + name), root / name
                a.write_bytes(('new-' + name).encode())
                b.write_bytes(('old-' + name).encode())
                pairs.append((a, b))
            def fail():
                raise RuntimeError('late actual content rejection')
            with self.assertRaisesRegex(RuntimeError, 'late actual'):
                install(pairs, root, fail)
            for _, path in pairs:
                self.assertEqual(path.read_bytes(), ('old-' + path.name).encode())
            self.assertTrue((root / 'rejected-pair/component').exists())
            self.assertTrue((root / 'recovered-transaction.json').exists())

    def test_interrupted_pair_recovery_does_not_touch_equal_unmoved_old_object(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage, dest = root / 'stage', root / 'component'
            stage.write_bytes(b'same')
            dest.write_bytes(b'same')
            pin = {'bytes': 4, 'sha256': sha(b'same')}
            (root / 'component-transaction.json').write_bytes(canonical({'schema': 'polaris-mobile-pair-transaction-v1',
                'entries': [{'stage': str(stage), 'destination': str(dest), 'name': dest.name,
                             'stagePin': pin, 'originalPin': pin}]}))
            recover([(stage, dest)], root)
            self.assertEqual(dest.read_bytes(), b'same')
            self.assertFalse((root / 'rejected-pair').exists())

    def test_occupied_recovery_slot_is_preserved_without_new_install(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'previous-pair').mkdir()
            unique = root / 'previous-pair/unique-object'
            unique.write_bytes(b'original')
            stage = root / 'stage'
            stage.write_bytes(b'new')
            with self.assertRaises(RuntimeError):
                install([(stage, root / 'component')], root, lambda: None)
            self.assertEqual(unique.read_bytes(), b'original')
            self.assertEqual(stage.read_bytes(), b'new')

    def test_source_fetch_is_fixed_tag_object_and_checked_before_handoff(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / 'source'
            calls = []
            with patch('ci_android.run', side_effect=lambda args, **kwargs: calls.append([str(x) for x in args])), \
                    patch('ci_android.verify_source', return_value={'sourceOverlay': 0}) as verify:
                prepare_source(source, self.value)
            ref = 'refs/tags/' + self.value['sourceTag']
            self.assertEqual(calls[1], ['git', '-C', str(source), 'fetch', '--depth=1', '--no-tags',
                                      self.value['sourceURL'], self.value['sourceTagObject'] + ':' + ref])
            verify.assert_called_once_with(source, self.value)
            for error in ['tag', 'object', 'commit', 'tree', 'module', 'dirty', 'overlay', 'generator']:
                with self.subTest(error=error), patch('ci_android.run'), \
                        patch('ci_android.verify_source', side_effect=RuntimeError(error)), self.assertRaises(RuntimeError):
                    prepare_source(source, self.value)

    def test_cache_output_refuses_invalid_identity_before_any_write(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            out, env = root / 'output', root / 'env'
            for p in (out, env):
                p.write_text('retained\n')
            with patch.dict(os.environ, {'GITHUB_OUTPUT': str(out), 'GITHUB_ENV': str(env)}):
                for bad in ['', 'A' * 64, 'g' * 64, 'a' * 63, 'a' * 65]:
                    with self.subTest(fp=bad), self.assertRaises(RuntimeError):
                        emit_outputs({'work': '/work', 'cache': '/cache'}, bad, root)
                    self.assertEqual(out.read_text(), 'retained\n')
                with self.assertRaises(RuntimeError):
                    emit_outputs({'work': '/work\nINJECT=1', 'cache': '/cache'}, 'a' * 64, root)
                self.assertEqual(env.read_text(), 'retained\n')

    def test_shared_budget_does_not_sell_the_same_free_bytes_twice(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            value = deepcopy(self.value)
            value['budget']['buildPeakReserveBytes'] = 1024
            active = []
            with patch.dict(os.environ, {'GITHUB_ACTIONS': '', 'POLARIS_BUILD_BUDGET_ROOT': str(root / 'budget')}, clear=True), \
                    patch('budget.os.statvfs', return_value=type('Space', (), {'f_bavail': 1, 'f_frsize': 2048})()), \
                    self.assertRaises(RuntimeError):
                with reservation(root / 'work', value, active):
                    self.fail('no peak/system margin')
            self.assertEqual(active, [])

    def test_hosted_admission_has_real_descriptor_and_nested_child_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); root.chmod(0o700)
            value = deepcopy(self.value); value['budget']['buildPeakReserveBytes'] = 1024
            active, nested = [], []
            with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true',
                    'POLARIS_BUILD_BUDGET_ROOT': str(root / 'budget')}, clear=True), \
                    patch('budget.os.statvfs', return_value=type('Space', (), {'f_bavail': 10**12, 'f_frsize': 1})()):
                with reservation(root / 'work', value, active):
                    fd = int(os.environ['POLARIS_BUILD_RESERVATION_FD'])
                    self.assertEqual(active, [fd])
                    self.assertTrue(os.get_inheritable(fd))
                    row = json.loads(os.pread(fd, 8192, 0))
                    self.assertEqual(row['reserveBytes'], 1024)
                    self.assertEqual(row['filesystem'], (root / 'work').stat().st_dev)
                    with reservation(root / 'child-work', value, nested):
                        self.assertEqual(nested, [fd])
                        self.assertEqual(int(os.environ['POLARIS_BUILD_RESERVATION_FD']), fd)
                    self.assertEqual(nested, [])
                    lock = root / 'budget' / row['slot']; lock.chmod(0o644)
                    try:
                        with self.assertRaisesRegex(RuntimeError, 'identity differs'):
                            with reservation(root / 'child-work', value, nested):
                                self.fail('unprotected inherited descriptor admitted')
                    finally:
                        lock.chmod(0o600)
                self.assertEqual(active, [])
                self.assertNotIn('POLARIS_BUILD_RESERVATION_FD', os.environ)
                with self.assertRaises(OSError):
                    os.fstat(fd)

    def test_prepare_refusal_never_emits_success_outputs(self):
        value = load()
        for installer_refuses in [True, False]:
            with self.subTest(installer_refuses=installer_refuses), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary).resolve()
                sdk, ndk, jdk = (root / name for name in ('sdk', 'ndk', 'jdk'))
                for path in (sdk, ndk, jdk):
                    path.mkdir()
                output, environment = root / 'github-output', root / 'github-env'
                installer = ['node', str(ROOT / 'scripts/mobile-core/prepare-ndk-linux.mjs'), '--profile',
                             str(ROOT / value['android']['ndkArchivePath']), '--sdk', str(sdk)]

                def controlled_run(args, **kwargs):
                    command = [str(arg) for arg in args]
                    if command == ['which', 'go']:
                        return '/controlled/go'
                    if command == ['/controlled/go', 'version']:
                        return 'go version go' + value['goVersion'] + ' linux/amd64'
                    if command == ['/controlled/go', 'install',
                            'github.com/sagernet/gomobile/cmd/gomobile@' + value['gomobileVersion'],
                            'github.com/sagernet/gomobile/cmd/gobind@' + value['gobindVersion']]:
                        return ''
                    if command == installer + ['--check-license-only']:
                        return ''
                    if command == installer + [str(root)]:
                        if installer_refuses:
                            raise RuntimeError('controlled NDK refusal')
                        return str(ndk)
                    self.fail('unexpected external operation: ' + repr(command))

                reason = 'controlled NDK refusal' if installer_refuses else 'controlled source refusal'
                with patch.dict(os.environ, {'GITHUB_ACTIONS': 'true', 'JAVA_HOME_17_X64': str(jdk),
                        'ANDROID_SDK_ROOT': str(sdk), 'GITHUB_OUTPUT': str(output), 'GITHUB_ENV': str(environment)}, clear=True), \
                        patch('ci_android.location', return_value=root), \
                        patch('ci_android.producer_code', return_value={'files': []}), \
                        patch('ci_android.native_capacity', return_value={'jobCap': 1}), \
                        patch('ci_android.native_environment', side_effect=lambda env, observation: (dict(env), observation)), \
                        patch('ci_android.run', side_effect=controlled_run), \
                        patch('ci_android.prepare_source', side_effect=RuntimeError('controlled source refusal')) as source, \
                        self.assertRaisesRegex(RuntimeError, reason):
                    prepare()
                if installer_refuses:
                    source.assert_not_called()
                else:
                    source.assert_called_once()
                self.assertFalse(output.exists())
                self.assertFalse(environment.exists())
                self.assertFalse((root / 'tools.json').exists())

    def test_framework_declares_exact_device_and_simulator_architectures(self):
        device = {'SupportedPlatform': 'ios', 'SupportedArchitectures': ['arm64']}
        simulator = {**device, 'SupportedPlatformVariant': 'simulator'}
        self.assertEqual(framework_entry_targets(device), {'ios-arm64'})
        self.assertEqual(framework_entry_targets(simulator), {'ios-arm64-simulator'})
        for bad in [{**device, 'SupportedArchitectures': []}, {**device, 'SupportedArchitectures': ['arm64', 'x86_64']},
                    {**simulator, 'SupportedArchitectures': ['arm64', 'x86_64']},
                    {**simulator, 'SupportedArchitectures': ['x86_64']},
                    {**simulator, 'SupportedArchitectures': ['arm64', 'arm64']},
                    {**simulator, 'SupportedArchitectures': ['arm64', 'x86_64', 'armv7']},
                    {**device, 'SupportedPlatform': 'macos'},
                    {**simulator, 'SupportedPlatformVariant': 'maccatalyst'}]:
            with self.subTest(entry=bad), self.assertRaises(RuntimeError):
                framework_entry_targets(bad)

    def test_llvm_symlink_drivers_keep_invocation_name_and_bind_actual_physical_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            drivers = {'ld.lld': 'lld', 'clang++': 'clang', 'llvm-ar': 'llvm-ar-physical',
                       'llvm-readelf': 'llvm-readobj'}
            for name, physical_name in drivers.items():
                physical = root / physical_name
                physical.write_bytes(('inert physical ' + physical_name).encode())
                driver = root / name; driver.symlink_to(physical_name)
                calls = []
                def observe(command, *, env):
                    calls.append(command)
                    self.assertEqual(command, [str(driver), '--version'])
                    self.assertEqual(env, {'controlled': 'input'})
                    return name + ' controlled version'
                with patch('producer.run', side_effect=observe):
                    fact = compiler_fact(driver, {'controlled': 'input'})
                self.assertEqual(calls, [[str(driver), '--version']])
                self.assertEqual(fact['path'], str(physical))
                self.assertEqual(fact['driverPath'], str(driver))
                self.assertEqual(fact['driverLinkTarget'], physical_name)
                self.assertEqual(fact['sha256'], sha(physical.read_bytes()))
                self.assertEqual(fact['bytes'], physical.stat().st_size)
                self.assertEqual(fact['version'], name + ' controlled version')

    def test_compiler_observation_refuses_retargeted_alias_or_changed_physical_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            physical, replacement, driver = root / 'lld', root / 'other-lld', root / 'ld.lld'
            physical.write_bytes(b'inert same physical'); replacement.write_bytes(physical.read_bytes())
            driver.symlink_to('lld')
            def retarget(*args, **kwargs):
                driver.unlink(); driver.symlink_to('other-lld')
                return 'controlled version'
            with patch('producer.run', side_effect=retarget), self.assertRaisesRegex(RuntimeError, 'changed during observation'):
                compiler_fact(driver, {})
            driver.unlink(); driver.symlink_to('lld')
            def change(*args, **kwargs):
                physical.write_bytes(b'changed actual physical')
                return 'controlled version'
            with patch('producer.run', side_effect=change), self.assertRaisesRegex(RuntimeError, 'changed during observation'):
                compiler_fact(driver, {})
            driver.unlink(); driver.symlink_to('missing-lld')
            with self.assertRaises(OSError):
                executable_driver_identity(driver)

    def relocated_android_observations(self):
        name = 'jni/arm64-v8a/libbox.so'
        body = '\tpath\tgobind\n\tdep\texample.org/module\tv1.2.3\th1:fixed\n'
        build_info = ': go' + self.value['goVersion'] + '\n' + body
        original = {'format': 'AAR', 'artifact': {'bytes': 123, 'sha256': 'a' * 64},
                    'nativeLibraries': {name: {'bytes': 42, 'sha256': 'b' * 64,
                        'goBuildInfo': '/producer/scratch/jni_arm64-v8a_libbox.so' + build_info,
                        'parsedBuildInfo': self.facts(), 'buildID': 'original-build-id',
                        'jniExports': ['JNI_OnLoad'], 'needed': ['libc.so']}},
                    'javaInterfaces': {'Libbox': 'original API'}}
        observed = deepcopy(original)
        observed['nativeLibraries'][name]['goBuildInfo'] = '/consumer/work/jni_arm64-v8a_libbox.so' + build_info
        return name, original, observed

    def test_component_identity_ignores_only_the_exact_go_invocation_filename_and_preserves_raw(self):
        name, original, observed = self.relocated_android_observations()
        frozen = deepcopy(original), deepcopy(observed)
        self.assertNotEqual(original, observed)
        self.assertEqual(component_output_identity(original), component_output_identity(observed))
        self.assertEqual((original, observed), frozen, 'original stdout and receipt observations must stay intact')
        for mutate in [
            lambda v: v['artifact'].update(sha256='c' * 64),
            lambda v: v['nativeLibraries'][name].update(bytes=43),
            lambda v: v['nativeLibraries'][name].update(buildID='other-build-id'),
            lambda v: v['nativeLibraries'][name].update(jniExports=[]),
            lambda v: v['nativeLibraries'][name].update(needed=['libother.so']),
            lambda v: v['nativeLibraries'][name]['parsedBuildInfo']['settings'].update(GOARCH='amd64'),
            lambda v: v['nativeLibraries'][name]['parsedBuildInfo']['modules']['example.org/module'].update(sum='h1:other'),
            lambda v: v['nativeLibraries'][name].update(goBuildInfo=v['nativeLibraries'][name]['goBuildInfo'].replace('h1:fixed', 'h1:other')),
            lambda v: v['javaInterfaces'].update(Libbox='changed API'),
            lambda v: v.update(unboundNull=None),
        ]:
            wrong = deepcopy(observed); mutate(wrong)
            self.assertNotEqual(component_output_identity(original), component_output_identity(wrong))
        version = ': go' + self.value['goVersion']
        for header in ['relative/jni_arm64-v8a_libbox.so' + version,
                       '/consumer/work/other.so' + version,
                       '/consumer/work/jni_arm64-v8a_libbox.so: go1.25.6']:
            wrong = deepcopy(observed)
            wrong['nativeLibraries'][name]['goBuildInfo'] = header + '\n\tpath\tgobind\n'
            with self.subTest(header=header), self.assertRaises(RuntimeError):
                component_output_identity(wrong)
        bad = deepcopy(observed); bad['nativeLibraries'][name]['goBuildInfo'] += '\t=>\t./other-source\t(devel)\n'
        self.assertNotEqual(component_output_identity(original), component_output_identity(bad))

    def test_non_aar_identity_borrows_unmodified_output_without_json_roundtrips(self):
        framework = {'format': 'static-XCFramework', 'goBuildInfo': {'moduleRaw': 'raw original'},
                     'slices': {'ios-arm64': {'members': [{'name': 'go.o', 'sha256': 'a' * 64}]}}}
        frozen = deepcopy(framework)
        with patch.object(artifacts.json, 'dumps', side_effect=AssertionError('redundant JSON serialization')), \
                patch.object(artifacts.json, 'loads', side_effect=AssertionError('redundant JSON parsing')):
            self.assertIs(component_output_identity(framework), framework)
            self.assertEqual(component_output_identity(framework), frozen)
            for field in ('goBuildInfo', 'slices'):
                changed = deepcopy(framework)
                changed[field] = {'changed': True}
                self.assertNotEqual(component_output_identity(framework), component_output_identity(changed))
        self.assertEqual(framework, frozen)

    def test_apple_identity_reuse_keeps_each_consumer_and_producer_reinspection(self):
        from consume import verify_original
        from producer import verify_receipt
        output = {'format': 'static-XCFramework', 'inventory': [{'path': 'original', 'sha256': 'a' * 64}],
                  'slices': {target: {'members': [{'name': 'go.o', 'sha256': 'b' * 64}]}
                             for target in self.value['apple']['slices']}}
        binding = {'platform': 'apple', 'sourceContract': self.value, 'sourceOverlay': 0,
                   'sourceObservation': {'sourceCommit': self.value['sourceCommit'], 'sourceTree': self.value['sourceTree']},
                   'moduleGraph': []}
        receipt = {'schema': 'polaris-mobile-component-v1', 'platform': 'apple', 'input': binding,
                   'output': output, 'nativeInputs': {}, 'sourceOverlay': 0, 'AppAccepted': False, 'devicePassed': False}
        receipt['fingerprint'] = artifacts.receipt_digest(receipt)
        frozen, observed = deepcopy(receipt), deepcopy(output)
        with patch('consume.load', return_value=self.value), patch('consume.read_receipt', return_value=receipt), \
                patch('consume.inspect_framework', return_value=observed) as consumer_inspection, \
                patch('producer.inspect_framework', return_value=observed) as producer_inspection, \
                patch('native_inputs.verify') as original_inputs:
            for _ in range(2):
                self.assertIs(verify_original('/Libbox.xcframework', '/receipt.json', 'apple', {}, Path('/scratch')), receipt)
                self.assertIs(verify_receipt('/Libbox.xcframework', receipt, self.value, binding, {}, None, Path('/scratch')), receipt)
            for field, changed in [('inventory', []), ('slices', {})]:
                observed[field] = changed
                with self.assertRaisesRegex(RuntimeError, 'actual component bytes/API/native identity differ'):
                    verify_original('/Libbox.xcframework', '/receipt.json', 'apple', {}, Path('/scratch'))
                with self.assertRaisesRegex(RuntimeError, 'cached component native/content'):
                    verify_receipt('/Libbox.xcframework', receipt, self.value, binding, {}, None, Path('/scratch'))
                observed[field] = deepcopy(output[field])
            self.assertEqual(consumer_inspection.call_count, 4)
            self.assertEqual(producer_inspection.call_count, 4)
            self.assertEqual(original_inputs.call_count, 8)
        self.assertEqual(receipt, frozen)

    def test_consumer_and_producer_reinspection_keep_the_original_receipt_identity(self):
        from consume import verify_original
        from producer import verify_receipt
        name, original, observed = self.relocated_android_observations()
        binding = {'platform': 'android', 'sourceContract': self.value, 'sourceOverlay': 0,
                   'sourceObservation': {'sourceCommit': self.value['sourceCommit'], 'sourceTree': self.value['sourceTree']},
                   'moduleGraph': []}
        receipt = {'schema': 'polaris-mobile-component-v1', 'platform': 'android', 'input': binding,
                   'output': original, 'nativeInputs': {}, 'sourceOverlay': 0, 'AppAccepted': False, 'devicePassed': False}
        receipt['fingerprint'] = sha(canonical(receipt))
        frozen = deepcopy(receipt)
        tools = {'go': 'actual-go', 'javap': 'actual-javap', 'readelf': 'actual-readelf'}
        with patch('consume.load', return_value=self.value), patch('consume.read_receipt', return_value=receipt), \
                patch('consume.inspect_aar', return_value=observed), patch('native_inputs.verify'), \
                patch('producer.inspect_aar', return_value=observed):
            self.assertIs(verify_original('/component.aar', '/receipt.json', 'android', tools, Path('/consumer/work')), receipt)
            self.assertIs(verify_receipt('/component.aar', receipt, self.value, binding, tools, None, Path('/consumer/work')), receipt)
            self.assertEqual(receipt, frozen)
            observed['nativeLibraries'][name]['buildID'] = 'changed-native-id'
            with self.assertRaisesRegex(RuntimeError, 'native identity differ; fields=.*nativeLibraries.*originalRawHeaders='):
                verify_original('/component.aar', '/receipt.json', 'android', tools, Path('/consumer/work'))
            with self.assertRaisesRegex(RuntimeError, 'cached component native/content'):
                verify_receipt('/component.aar', receipt, self.value, binding, tools, None, Path('/consumer/work'))

if __name__ == '__main__':
    unittest.main()
