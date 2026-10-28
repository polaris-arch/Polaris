"""Linux VM evidence helpers. Import is inert; unit fixtures use injected readers/children.

The owner signals only its own unreaped Popen child, never a supplied PID/group.
This bounds the app/X server roots, not arbitrary detached descendants.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import signal
import sys
import time

FLAGS = ('WEBKIT_DISABLE_COMPOSITING_MODE', 'WEBKIT_DISABLE_DMABUF_RENDERER', 'LIBGL_ALWAYS_SOFTWARE')


def graphics_environment(inherited, leg):
    env = dict(inherited)
    for key in FLAGS:
        env.pop(key, None)
    if leg == 'escape':
        env.update({key: '1' for key in FLAGS})
    elif leg != 'default':
        raise ValueError('unknown graphics leg')
    return env


def proc(pid):
    root = Path('/proc') / str(pid)
    fields = (root / 'stat').read_text().rsplit(') ', 1)[1].split()
    if fields[0] in ('Z', 'X'):
        raise ValueError('dead process')
    env = dict(item.split('=', 1) for item in (root / 'environ').read_bytes().decode().split('\0') if '=' in item)
    return dict(identity=f'{pid}:{fields[19]}', parent=int(fields[1]),
                executable=(root / 'exe').readlink().name, env=env)


def graphics_sample(identity, display, leg, pids, read=proc):
    """Re-read every ancestry identity after collection; races fail closed."""
    root_pid = int(identity.split(':')[0])
    root = read(root_pid)
    expected = {key: '1' for key in FLAGS} if leg == 'escape' else {}
    if root['identity'] != identity or root['env'].get('DISPLAY') != display:
        raise ValueError('app identity/display changed')
    effective = {key: root['env'][key] for key in FLAGS if key in root['env']}
    if effective != expected:
        raise ValueError('effective graphics environment differs')
    witnesses = []
    for pid in pids:
        try:
            child = read(pid)
            if child['executable'] != 'WebKitWebProcess' or child['env'].get('DISPLAY') != display:
                continue
            if {key: child['env'][key] for key in FLAGS if key in child['env']} != expected:
                continue
            chain, seen, current = [], set(), child
            while current['identity'] != identity:
                if current['identity'] in seen or len(chain) >= 64:
                    raise ValueError('unowned ancestry')
                seen.add(current['identity'])
                chain.append(current)
                current = read(current['parent'])
            chain.append(root)
            if any(int(parent['identity'].split(':')[1]) > int(child['identity'].split(':')[1])
                   for child, parent in zip(chain, chain[1:])):
                raise ValueError('reused ancestor PID')
            if any(read(int(item['identity'].split(':')[0])) != item for item in chain):
                raise ValueError('ancestry changed')
            witnesses.append([item['identity'] for item in chain])
        except (OSError, ValueError, KeyError, UnicodeError):
            continue
    if read(root_pid)['identity'] != identity:
        raise ValueError('app changed during sample')
    return dict(identity=identity, display=display, leg=leg, effective=effective, witnesses=witnesses)


def settle(child, grace=5):
    # A direct, unreaped child cannot have its PID recycled. Popen also polls
    # before signalling. There is no arbitrary PID/process-group signal path.
    if child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=grace)
        except subprocess.TimeoutExpired:
            child.kill()
    try:
        child.wait(timeout=grace)
        return True
    except subprocess.TimeoutExpired:
        return False  # e.g. uninterruptible kernel sleep; never claim cleanup


def live_identity(identity, read=proc):
    current = read(int(identity.split(':')[0]))
    if current['identity'] != identity:
        raise ValueError('owned root birth identity changed')
    return identity


def owned_live(out, label):
    return live_identity((out / f'{label}.identity').read_text())


def monitor_owned(child, identity, budget, cancelled, clock=time.monotonic, pause=time.sleep, read=proc):
    """Observe the held child's lifetime; reap alone is never a successful run."""
    deadline = clock() + budget
    reason = 'identity_unavailable'
    try:
        while True:
            # Natural exit takes precedence over a late cancellation request.
            if child.poll() is not None:
                reason = 'natural_exit'
                break
            live_identity(identity, read)
            if cancelled():
                reason = 'cancelled'
                break
            if clock() >= deadline:
                reason = 'deadline'
                break
            pause(0.1)
    except (OSError, ValueError, KeyError, UnicodeError):
        reason = 'identity_unavailable'
    finally:
        clean = settle(child)
    return dict(root_reaped=clean, identity=identity, reason=reason, exit_code=child.returncode)


def own(out, label, budget, command):
    if not 1 <= budget <= 7200 or label not in ('app', 'xvfb'):
        raise ValueError('finite owner budget required')
    stopping = False
    def stop(_signum, _frame):
        nonlocal stopping
        stopping = True
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    child = subprocess.Popen(command, env=graphics_environment(os.environ, os.environ['GRAPHICS']), start_new_session=True)
    identity = ''
    try:
        identity = proc(child.pid)['identity']
        (out / f'{label}.identity').write_text(identity)
        (out / f'{label}.pid').write_text(str(child.pid))
        receipt = monitor_owned(child, identity, budget,
                                lambda: stopping or (out / f'{label}.cancel').exists())
    except (OSError, ValueError, KeyError, UnicodeError):
        receipt = dict(root_reaped=settle(child), identity=identity,
                       reason='identity_unavailable', exit_code=child.returncode)
    (out / f'{label}.cleanup.json').write_text(json.dumps(receipt))
    return 0 if receipt['root_reaped'] and receipt['reason'] == 'cancelled' else 2


def provenance(out, batch, leg):
    if len(batch) != 32 or any(c not in '0123456789abcdef' for c in batch):
        raise ValueError('BATCH_ID must be a fresh shared 32-hex nonce')
    source = hashlib.sha256()
    for name in ('run.sh', 'assert.py', 'scenario.py', 'evidence_guard.py'):
        source.update(name.encode() + b'\0' + Path(__file__).with_name(name).read_bytes())
    config = json.loads((out / 'config.json').read_text())
    for key in ('servers', 'selectedServerId'):
        config.pop(key, None)
    return dict(batch=batch, boot_id=Path('/proc/sys/kernel/random/boot_id').read_text().strip(), graphics=leg, source_sha256=source.hexdigest(),
                binary_sha256=hashlib.sha256(Path('/usr/bin/polaris').read_bytes()).hexdigest(),
                common_config_sha256=hashlib.sha256(json.dumps(config, sort_keys=True).encode()).hexdigest())


def main():
    mode, directory, *args = sys.argv[1:]
    out = Path(directory)
    if mode == 'own':
        return own(out, args[0], int(args[1]), args[2:])
    if mode == 'live':
        print(owned_live(out, args[0]))
    elif mode == 'sample':
        server_identity = owned_live(out, 'xvfb')
        sample = graphics_sample(args[0], os.environ['DISPLAY'], os.environ['GRAPHICS'],
                                 [int(p.name) for p in Path('/proc').iterdir() if p.name.isdigit()])
        if owned_live(out, 'xvfb') != server_identity:
            raise ValueError('owned display root changed during sample')
        sample['xvfb_identity'] = server_identity
        sample['elapsed'] = args[1]
        with (out / 'graphics-samples.jsonl').open('a') as stream:
            stream.write(json.dumps(sample) + '\n')
        print(len(sample['witnesses']))
    elif mode == 'provenance':
        value = provenance(out, os.environ['BATCH_ID'], os.environ['GRAPHICS'])
        target = out / 'provenance.json'
        if target.exists():
            if json.loads(target.read_text()) != value:
                raise ValueError('source/binary/config changed during run')
        else:
            target.write_text(json.dumps(value) + '\n')
    else:
        raise ValueError('unknown mode')
    return 0


if __name__ == '__main__':
    sys.exit(main())
