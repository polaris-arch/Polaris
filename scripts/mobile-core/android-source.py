#!/usr/bin/env python3
"""Go BuildInfo parsing and canonical digests shared by mobile-core carrier inspection."""
import hashlib
import json
import re


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def parse_build_info(raw):
    lines = raw.rstrip().splitlines()
    version = re.search(r': go(\d+\.\d+\.\d+)$', lines.pop(0) if lines else '')
    require(version, 'Missing Go BuildInfo')
    facts = {'goVersion': version[1], 'modules': {}, 'settings': {}, 'path': None}
    previous = None
    for line in lines:
        if line == '\t':
            previous = None
            continue
        columns = line.split('\t')
        require(columns[0] == '' and len(columns) >= 3, 'Malformed Go BuildInfo row')
        kind, value = columns[1:3]
        if kind in ('mod', 'dep'):
            require(len(columns) >= 4 and value not in facts['modules'], 'Duplicate/malformed module row')
            previous = {'kind': kind, 'version': columns[3], **({'sum': columns[4]} if len(columns) > 4 and columns[4] else {})}
            facts['modules'][value] = previous
        elif kind == '=>':
            require(previous is not None and 'replacement' not in previous and value, 'Unbound/duplicate replacement')
            previous['replacement'] = value
            if len(columns) > 3 and columns[3]:
                previous['replacementVersion'] = columns[3]
            if len(columns) > 4 and columns[4]:
                previous['replacementSum'] = columns[4]
            previous = None
        elif kind == 'build':
            key, sep, setting = value.partition('=')
            require(sep and key and key not in facts['settings'], 'Malformed/duplicate build setting')
            facts['settings'][key] = json.loads(setting) if setting.startswith('"') else setting
            previous = None
        elif kind == 'path':
            require(facts['path'] is None, 'Duplicate BuildInfo path')
            facts['path'], previous = value, None
        else:
            raise RuntimeError('Unsupported Go BuildInfo row')
    return facts
