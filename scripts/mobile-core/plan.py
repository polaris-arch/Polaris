#!/usr/bin/env python3
"""Read/verify formal mobile inputs and emit an explicit bind plan, never execute it.

The producer runner and artifact/carrier predicates consume this same plan.
Source planning does not issue a component, approval or a device receipt.
"""
import argparse
import json
from pathlib import Path

from fork_input import bind_command, canonical, load, sha, verify_source


def plan(platform, value, *, gomobile, output, bootclasspath=None):
    target = value[platform]
    facts = {'schema': 'polaris-mobile-bind-input-v1', 'source': value,
             'platform': platform, 'targets': target['targets'], 'tags': target['tags'],
             'gomobileVersion': value['gomobileVersion'], 'gobindVersion': value['gobindVersion'],
             'artifactAcceptance': False}
    fingerprint = sha(canonical(facts))
    return {'schema': 'polaris-mobile-bind-plan-v1', 'facts': facts, 'fingerprint': fingerprint,
            'command': bind_command(platform, value, gomobile, output, fingerprint, bootclasspath),
            'sourceOverlay': 0, 'actualToolIdentityRequiredBeforeExecution': True,
            'requiredOutputChecks': (['four exact ABI native ELF', 'API24 Android ident',
                '16KiB PT_LOAD', 'complete JNI/Java API', 'actual Go BuildInfo/module/replacement',
                'actual BuildID and source/tool input binding', 'final APK/R8/alignment']
                if platform == 'android' else ['three exact iOS architecture slices',
                'static archives and generated Objective-C/Swift API', 'source/module/runtime inputs',
                'actual SDK/compiler identities and BuildID', 'final App/PacketTunnel static-link provenance']),
            'producerReceipt': None, 'AppAccepted': False, 'devicePassed': False,
            'publicationEligible': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('platform', choices=['android', 'apple'])
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--gomobile', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--bootclasspath', type=Path)
    args = parser.parse_args()
    value = load()
    source = verify_source(args.source, value)
    result = plan(args.platform, value, gomobile=args.gomobile, output=args.output,
                  bootclasspath=args.bootclasspath)
    result['sourceObservation'] = source
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
