import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import apple_tls_fingerprint as subject


class OptionalTLSCapabilityLink(unittest.TestCase):
    def test_old_core_and_matched_new_c_abi(self):
        self.assertFalse(subject.capability_present(['NSString* LibboxVersion(void);'], 'T _LibboxVersion\n'))
        header = 'NSString* _Nonnull LibboxTLSFingerprintCapabilities(NSError* _Nullable* _Nullable error);'
        self.assertTrue(subject.capability_present([header], '000000001 T ' + subject.SYMBOL + '\n'))
        for headers, symbols in [([header], ''), ([], 'T ' + subject.SYMBOL + '\n'),
                                 (['void LibboxTLSFingerprintCapabilities(void);'], 'T ' + subject.SYMBOL + '\n')]:
            with self.assertRaises(RuntimeError):
                subject.capability_present(headers, symbols)

    def test_final_definition_without_dynamic_export_is_not_accepted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            executable = root / 'extension'
            executable.write_bytes(b'fixture native bytes')
            flags = root / 'PolarisTLSFingerprintLinker.args'
            flags.write_text('-Xlinker\n-u\n-Xlinker\n' + subject.SYMBOL + '\n')
            env = {'DERIVED_FILE_DIR': tmp, 'TARGET_BUILD_DIR': tmp, 'EXECUTABLE_PATH': 'extension'}
            receipt = {'capabilityPresent': True, 'flags': subject.file_pin(flags)}
            with patch.dict(os.environ, env), patch.object(subject, 'read_receipt', return_value=receipt):
                with patch.object(subject, 'run', side_effect=['T ' + subject.SYMBOL + '\n', 'unrelated export']):
                    with self.assertRaisesRegex(RuntimeError, 'dynamically discoverable'):
                        subject.verify()
                with patch.object(subject, 'run', return_value=''):
                    with self.assertRaisesRegex(RuntimeError, 'stripped'):
                        subject.verify()


if __name__ == '__main__':
    unittest.main()
