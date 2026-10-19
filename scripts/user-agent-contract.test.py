"""Lightweight source/provenance gates; Rust behavior tests run in normal CI."""
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    return (ROOT / path).read_text()


class UserAgentContract(unittest.TestCase):
    def test_desktop_uses_admitted_selection(self):
        source = read('src-tauri/src/runtime/core_version.rs')
        self.assertIn('core_identity::current_platform()', source)
        self.assertIn('.and_then(crate::runtime::core_identity::selected_manifest)', source)
        self.assertIn('.map(|manifest| manifest.version)', source)
        self.assertNotIn('Command::', source)
        self.assertNotIn('core-manifest.json', source)
        self.assertIn('"unknown".to_owned()', source)

    def test_mobile_matches_linked_constant(self):
        source = read('src-tauri/src/runtime/core_version.rs')
        self.assertIn('scripts/mobile-core/source-contract.json', source)
        self.assertIn('contract["version"]', source)
        contract = json.loads(read('scripts/mobile-core/source-contract.json'))
        self.assertEqual(contract['schema'], 'polaris-mobile-fork-input-v1')
        self.assertEqual(contract['sourceTag'], 'v' + contract['version'])
        builder = read('scripts/mobile-core/fork_input.py')
        self.assertIn("constant.Version=' + value['version']", builder)

    def test_defaults_share_one_constructor(self):
        for path in ['src-tauri/src/commands/subscription.rs',
                     'src-tauri/src/commands/subscription/pipeline.rs']:
            source = read(path)
            self.assertIn('.unwrap_or_else(crate::runtime::http::app_user_agent)', source)
        ua = read('src-tauri/src/runtime/http/user_agent.rs')
        updater = read('src-tauri/src/runtime/updater.rs')
        self.assertIn('core_version::selected_bundled_version()', ua)
        self.assertIn('core_version::selected_bundled_version().to_owned()', updater)
        self.assertIn('static USER_AGENT: OnceLock<String>', ua)

    def test_custom_override_still_wins(self):
        pipeline = read('src-tauri/src/commands/subscription/pipeline.rs')
        self.assertIn('let ua = user_agent\n        .map(str::to_string)\n'
                      '        .unwrap_or_else(crate::runtime::http::app_user_agent);', pipeline)
        transport = read('src-tauri/src/runtime/http/subscription_transport.rs')
        self.assertIn('req.header(reqwest::header::USER_AGENT, init.user_agent)', transport)

    def test_rust_behavior_coverage_is_present(self):
        cases = read('crates/net-stack/src/subscription/tests/fetch_tests.rs')
        for marker in ['default_ua_includes_app_and_kernel_versions',
                       'default_ua_rejects_missing_or_unsafe_version_tokens',
                       r'\r\nInjected: yes', r'1\0', r'1\t2', 'repeat(129)']:
            self.assertIn(marker, cases)
        self.assertIn('fetch_identity_preserves_custom_ua_and_shares_default',
                      read('src-tauri/src/commands/subscription/tests/ua_tests.rs'))


if __name__ == '__main__':
    unittest.main()
