#!/usr/bin/env python3
"""Native CI only. Local tests inspect contracts, never execute a core or library.

The C API smoke follows Chromium's generated cronet.idl_c.h signatures:
https://chromium.googlesource.com/chromium/src/+/42bcd13e37f9b0a9efc540ae8feee9b4aae8e948/components/cronet/native/generated/cronet.idl_c.h
This is scoped smoke evidence, not a complete ABI or device verdict.
"""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tempfile
import time

REQUIRED = ['Cronet_Engine_Create', 'Cronet_EngineParams_Create', 'Cronet_Engine_StartWithParams',
            'Cronet_UrlRequest_Create', 'Cronet_UrlRequest_InitWithParams', 'Cronet_UrlRequest_Start']


def require(condition, message):
    if not condition:
        raise ValueError(message)


def ci_context(key, env=None, system=None, machine=None):
    env = os.environ if env is None else env
    system = platform.system() if system is None else system
    machine = platform.machine() if machine is None else machine
    branch = 'refs/heads/collab/fk02-native-payload-validation-20261010'
    require(env.get('POLARIS_NO_KERNEL_RUN') != '1' and env.get('POLARIS_NATIVE_PAYLOAD_VALIDATION') == '1'
            and env.get('GITHUB_ACTIONS') == 'true' and env.get('GITHUB_EVENT_NAME') == 'workflow_dispatch'
            and env.get('GITHUB_REPOSITORY') == 'polaris-arch/Polaris' and env.get('GITHUB_REF') == branch
            and env.get('GITHUB_WORKFLOW_REF') == 'polaris-arch/Polaris/.github/workflows/release-risk.yml@' + branch
            and re.fullmatch('[a-f0-9]{40}', env.get('GITHUB_SHA', ''))
            and re.fullmatch('[1-9][0-9]*', env.get('GITHUB_RUN_ID', ''))
            and re.fullmatch('[1-9][0-9]*', env.get('GITHUB_RUN_ATTEMPT', '')),
            'native payload requires exact non-tag manual CI validation context')
    require(key in ['linux', 'win'] and system == {'linux': 'Linux', 'win': 'Windows'}[key]
            and machine.lower() in ['x86_64', 'amd64'], 'native amd64 host/platform differs')
    return {'candidate': env['GITHUB_SHA'], 'runId': env['GITHUB_RUN_ID'], 'attempt': env['GITHUB_RUN_ATTEMPT']}


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def regular(path):
    path = Path(os.path.abspath(path))
    require(path.is_file() and not path.is_symlink(), 'non-regular native payload')
    for p in path.parents:
        require(p.is_dir() and not p.is_symlink()
                and not getattr(p, 'is_junction', lambda: False)(), 'native payload linked ancestor')
    return path


def linux_maps(text):
    paths = set()
    for line in text.splitlines():
        fields = line.split(None, 5)
        if len(fields) == 6 and fields[5].startswith('/'):
            path = re.sub(r'\\([0-7]{3})', lambda m: chr(int(m[1], 8)), fields[5])
            if Path(path.removesuffix(' (deleted)')).name == 'libcronet.so':
                require(not path.endswith(' (deleted)'), 'core maps deleted Cronet file')
                paths.add(path)
    return sorted(paths)


def windows_modules(pid):
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    psapi = ctypes.WinDLL('psapi', use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    psapi.EnumProcessModulesEx.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.HMODULE),
                                          wintypes.DWORD, ctypes.POINTER(wintypes.DWORD), wintypes.DWORD]
    psapi.GetModuleFileNameExW.argtypes = [wintypes.HANDLE, wintypes.HMODULE, wintypes.LPWSTR, wintypes.DWORD]
    handle = kernel.OpenProcess(0x0410, False, pid)
    require(handle, 'cannot inspect actual core process')
    try:
        modules = (wintypes.HMODULE * 4096)()
        needed = wintypes.DWORD()
        require(psapi.EnumProcessModulesEx(handle, modules, ctypes.sizeof(modules), ctypes.byref(needed), 3),
                'cannot enumerate actual core modules')
        require(needed.value <= ctypes.sizeof(modules), 'core module inventory truncated')
        paths = []
        for module in modules[:needed.value // ctypes.sizeof(wintypes.HMODULE)]:
            value = ctypes.create_unicode_buffer(32768)
            n = psapi.GetModuleFileNameExW(handle, module, value, len(value))
            require(0 < n < len(value), 'cannot read actual core module path')
            if Path(value.value).name.lower() == 'libcronet.dll':
                paths.append(value.value)
        return sorted(set(paths))
    finally:
        kernel.CloseHandle(handle)


def match_mapped(paths, expected, expected_sha):
    require(len(paths) == 1, 'actual core must map exactly one canonical Cronet library')
    mapped = regular(paths[0])
    require(mapped.samefile(expected) and sha(mapped) == expected_sha, 'actual core mapped other library bytes/path')
    return {'path': str(mapped), 'sha256': expected_sha}


def symbol_owner(address, key):
    if key == 'win':
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.GetModuleHandleExW.argtypes = [wintypes.DWORD, ctypes.c_void_p, ctypes.POINTER(wintypes.HMODULE)]
        kernel.GetModuleFileNameW.argtypes = [wintypes.HMODULE, wintypes.LPWSTR, wintypes.DWORD]
        handle = wintypes.HMODULE()
        require(kernel.GetModuleHandleExW(6, ctypes.c_void_p(address), ctypes.byref(handle)), 'cannot resolve symbol owner')
        path = ctypes.create_unicode_buffer(32768)
        n = kernel.GetModuleFileNameW(handle, path, len(path))
        require(0 < n < len(path), 'cannot read symbol owner')
        return path.value
    class DlInfo(ctypes.Structure):
        _fields_ = [('name', ctypes.c_char_p), ('base', ctypes.c_void_p),
                    ('symbol', ctypes.c_char_p), ('address', ctypes.c_void_p)]
    api = ctypes.CDLL('libdl.so.2')
    api.dladdr.argtypes = [ctypes.c_void_p, ctypes.POINTER(DlInfo)]
    info = DlInfo()
    require(api.dladdr(ctypes.c_void_p(address), ctypes.byref(info)) and info.name, 'cannot resolve symbol owner')
    return os.fsdecode(info.name)


def capi_smoke(library, key, expected_sha):
    ci_context(key)
    library = regular(library)
    require(sha(library) == expected_sha, 'C API input hash differs')
    lib = ctypes.CDLL(str(library), **({'winmode': 0x1100} if key == 'win' else {'mode': os.RTLD_NOW | os.RTLD_LOCAL}))
    symbols = {}
    for name in REQUIRED:
        address = ctypes.cast(getattr(lib, name), ctypes.c_void_p).value
        owner = regular(symbol_owner(address, key))
        require(owner.samefile(library) and sha(owner) == expected_sha, 'C API symbol resolved outside canonical library')
        symbols[name] = {'address': hex(address), 'owner': str(owner)}
    ptr = ctypes.c_void_p
    signatures = {'Cronet_Engine_Create': (ptr, []), 'Cronet_EngineParams_Create': (ptr, []),
                  'Cronet_Engine_StartWithParams': (ctypes.c_int, [ptr, ptr]),
                  'Cronet_Engine_Shutdown': (ctypes.c_int, [ptr]),
                  'Cronet_Engine_Destroy': (None, [ptr]), 'Cronet_EngineParams_Destroy': (None, [ptr]),
                  'Cronet_Engine_GetVersionString': (ctypes.c_char_p, [ptr])}
    for name, (result, args) in signatures.items():
        fn = getattr(lib, name); fn.restype = result; fn.argtypes = args
    engine = lib.Cronet_Engine_Create(); params = lib.Cronet_EngineParams_Create()
    require(engine and params, 'native C API allocation failed')
    try:
        require(lib.Cronet_Engine_StartWithParams(engine, params) == 0, 'native engine initialization failed')
        version = lib.Cronet_Engine_GetVersionString(engine)
        require(version, 'native Cronet version missing')
        require(lib.Cronet_Engine_Shutdown(engine) == 0, 'native engine shutdown failed')
    finally:
        lib.Cronet_Engine_Destroy(engine); lib.Cronet_EngineParams_Destroy(params)
    require(sha(library) == expected_sha, 'native library changed during smoke')
    return {'schema': 'polaris-cronet-capi-smoke-v1', 'librarySha256': expected_sha,
            'symbols': symbols, 'version': version.decode('ascii'), 'engineInitialized': True,
            'scope': 'six loader resolutions and engine create/start/shutdown; not full ABI'}


def core_probe(core, library, key, expected_sha):
    ci_context(key)
    core = regular(core); library = regular(library)
    # No inbound/TUN/listener, DNS, requests or external server names.
    config = {'log': {'level': 'info', 'timestamp': False}, 'outbounds': [
        {'type': 'naive', 'tag': 'probe', 'server': '127.0.0.1', 'server_port': 9,
         'username': 'probe', 'password': 'probe', 'tls': {'enabled': True, 'server_name': 'localhost'}}]}
    with tempfile.TemporaryDirectory(prefix='polaris-core-probe-') as tmp:
        tmp = Path(tmp); cfg = tmp / 'config.json'; log = tmp / 'core.log'
        cfg.write_text(json.dumps(config))
        env = os.environ.copy()
        for name in ['LD_PRELOAD', 'LD_LIBRARY_PATH', 'DYLD_LIBRARY_PATH', 'DYLD_INSERT_LIBRARIES']:
            env.pop(name, None)
        with log.open('wb') as out:
            process = subprocess.Popen([str(core), 'run', '-c', str(cfg)], cwd=core.parent,
                                       env=env, stdout=out, stderr=subprocess.STDOUT)
            mapped = None
            try:
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    require(process.poll() is None, 'naive core exited before startup: ' + log.read_text(errors='replace')[-4000:])
                    paths = windows_modules(process.pid) if key == 'win' else linux_maps(
                        Path(f'/proc/{process.pid}/maps').read_text())
                    if paths and 'sing-box started' in log.read_text(errors='replace'):
                        mapped = match_mapped(paths, library, expected_sha); break
                    time.sleep(0.1)
                require(mapped is not None, 'naive startup/canonical library mapping not observed')
            finally:
                if process.poll() is None:
                    process.terminate()
                    try: process.wait(timeout=5)
                    except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=5)
        require(sha(library) == expected_sha, 'mapped payload changed during core probe')
        return {'mappedLibrary': mapped, 'started': True, 'pid': process.pid,
                'configurationSha256': sha(cfg), 'log': log.read_text(errors='replace'),
                'scope': 'actual naive core initialization, no inbound/TUN/request/device acceptance'}


def observe_unsigned_windows(path):
    code = r'''$ErrorActionPreference='Stop'; $s=Get-AuthenticodeSignature -LiteralPath $args[0];
if ($s.Status -ne 'NotSigned') { throw "expected explicit unsigned distribution state: $($s.Status)" }
@{status=$s.Status.ToString(); signerThumbprint=$null} | ConvertTo-Json -Compress'''
    return json.loads(subprocess.check_output(['pwsh', '-NoProfile', '-NonInteractive', '-Command', code, str(path)], timeout=30))


def prove(root, key, core, library):
    context = ci_context(key)
    raw = subprocess.check_output(['node', str(root / 'scripts/native-payload-input.mjs'), key, str(core), str(library)],
                                  cwd=root, timeout=60)
    identity = json.loads(raw)
    signatures_before = {name: observe_unsigned_windows(path) for name, path in [('core', core), ('library', library)]} if key == 'win' else {'state': 'not-applicable'}
    require(identity['candidate'] == context['candidate'], 'native candidate differs')
    # Isolate C API calls: an ABI crash fails the child/run, never becomes a skip.
    capi = json.loads(subprocess.check_output([os.sys.executable, str(Path(__file__).resolve()),
                    '--capi-only', '--platform', key, '--library', str(library),
                    '--expected-sha256', identity['library']['sha256']], timeout=30))
    actual = core_probe(core, library, key, identity['library']['sha256'])
    require(sha(core) == identity['core']['sha256'], 'core changed during native probe')
    signatures_after = {name: observe_unsigned_windows(path) for name, path in [('core', core), ('library', library)]} if key == 'win' else {'state': 'not-applicable'}
    require(signatures_before == signatures_after, 'payload signature state changed during native proof')
    return {'schema': 'polaris-native-payload-proof-v1', 'transport': context,
            'input': identity, 'signatureBeforeNative': signatures_before, 'signatureAfterNative': signatures_after,
            'capi': capi, 'actualCore': actual}


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--platform', choices=['linux', 'win'], required=True)
    p.add_argument('--core', type=Path)
    p.add_argument('--library', type=Path, required=True)
    p.add_argument('--output', type=Path)
    p.add_argument('--capi-only', action='store_true')
    p.add_argument('--expected-sha256')
    a = p.parse_args(); ci_context(a.platform)
    if a.capi_only:
        require(re.fullmatch('[a-f0-9]{64}', a.expected_sha256 or ''), 'explicit library hash required')
        print(json.dumps(capi_smoke(a.library, a.platform, a.expected_sha256))); return
    require(a.core and a.output, 'core/output required')
    root = Path(__file__).resolve().parent.parent
    result = prove(root, a.platform, a.core.absolute(), a.library.absolute())
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
