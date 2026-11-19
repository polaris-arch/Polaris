#!/usr/bin/env python3
"""Observe optional PacketTunnel symbols under the existing native build lease."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

sys.dont_write_bytecode = True
from artifacts import file_pin
from fork_input import decode, require
from producer import run

SYMBOL = '_LibboxTLSFingerprintCapabilities'
# At most 2 MiB of selected headers, expanded by JSON escaping, plus pins.
SNAPSHOT_BYTES = 6 * (2 * 1024 ** 2) + 65536


def compiler_response(action, value=None):
    command = ['node', Path(__file__).resolve().with_name('apple-admission.mjs'), '--tls-link', action]
    payload = json.dumps(value, ensure_ascii=False).encode() if value is not None else b''
    result = subprocess.run(command, input=payload, capture_output=True, timeout=30)
    # Keep the existing context read observations in the native phase logs.
    sys.stderr.buffer.write(result.stderr)
    require(result.returncode == 0, 'TLS compiler response rejected')
    return decode(result.stdout, maximum=SNAPSHOT_BYTES) if result.stdout else None


def defined_in(binary):
    symbols = run(['xcrun', 'nm', '--no-llvm-bc', '-gU', binary])
    return bool(re.search(r'\bT\s+' + SYMBOL + r'\s*$', symbols, re.MULTILINE))


def prepare():
    inputs = compiler_response('select')
    compiler_response('prepare', {'input': inputs, 'defined': defined_in(inputs['binaryPath'])})


def verify():
    observation = compiler_response('verify-parent')
    executable = Path(os.environ['TARGET_BUILD_DIR']) / os.environ['EXECUTABLE_PATH']
    if observation['capabilityPresent']:
        require(defined_in(executable), 'final PacketTunnel stripped the actual TLS capability function')
        exports = run(['xcrun', 'dyld_info', '-exports', executable])
        require(re.search(r'(?<![\w])' + SYMBOL + r'(?![\w])', exports),
                'final PacketTunnel TLS capability function is not dynamically discoverable')
    print(json.dumps({'scope': 'actual-final-PacketTunnel-static-link',
                      'capabilityPresent': observation['capabilityPresent'], 'executable': file_pin(executable),
                      'nativeTunHeaderABIAvailable': observation['nativeTunHeaderABIAvailable'],
                      'loadedQueryExecuted': False, 'devicePassed': False}))


def native_action(action):
    from budget import reservation
    from producer import ACTIVE_FDS, private_dir
    from fork_input import load
    require(os.environ.get('POLARIS_BUILD_RESERVATION_FD') is not None and
            os.environ.get('POLARIS_FORMAL_APP_BUILD') == '1',
            'native TLS inspection must inherit admitted parent reservation')
    with reservation(private_dir(Path(os.environ['POLARIS_APP_WORK'])), load(), ACTIVE_FDS):
        {'prepare': prepare, 'verify': verify}[action]()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'verify'])
    native_action(parser.parse_args().action)
