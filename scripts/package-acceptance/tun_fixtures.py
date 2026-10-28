#!/usr/bin/env python3
"""Nonloopback adapter for the existing package fixtures; no namespace setup.

Run fixture roles only as an ordinary UID in an already registered backend
namespace. SOCKS relay accepts only the fixed isolated origin/health targets.
CLI uses stdin/stdout for private bounded control, never a public admin port.
"""
from __future__ import annotations
import argparse
import copy
import hashlib
import importlib.util
import ipaddress
import json
import os
import re
import subprocess
from pathlib import Path
import selectors
import signal
import socket
import struct
import sys
import threading
import time
from types import SimpleNamespace


def load_sibling(name):
    spec = importlib.util.spec_from_file_location('package_' + name, Path(__file__).with_name(name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# This file is ordinary-UID fixture code, never imported by the root watchdog.
base = load_sibling('fixtures')
tun = load_sibling('tun')
require, recv_exact = base.require, base.recv_exact
MAX_BYTES = 1024 * 1024


def parse_socks_request(conn):
    version, count = recv_exact(conn, 2)
    require(version == 5 and 0 < count <= 16 and 0 in recv_exact(conn, count), 'SOCKS no-auth greeting required')
    conn.sendall(b'\x05\x00')
    version, command, reserved, kind = recv_exact(conn, 4)
    require((version, command, reserved) == (5, 1, 0), 'CONNECT only')
    if kind == 1:
        host = socket.inet_ntop(socket.AF_INET, recv_exact(conn, 4))
    elif kind == 3:
        length = recv_exact(conn, 1)[0]
        require(0 < length <= 253, 'bounded domain required')
        host = recv_exact(conn, length).decode('ascii').lower()
    else:
        raise ValueError('IPv4/domain fixture only')
    return host, struct.unpack('!H', recv_exact(conn, 2))[0]


def allowed_target(plan, host, port):
    if host == base.DOMAIN:
        host = plan['origin']
    require(host == plan['origin'] and port in (plan['ports']['echo'], plan['ports']['health']),
            'isolated origin target only; no arbitrary proxy')
    return host, port


def dns_response(plan, packet, transport):
    name, kind, end = base.dns_question(packet)
    # Fresh nonce labels are allowed only below our reserved fixture domain.
    known = name == base.DOMAIN or name.endswith('.' + base.DOMAIN)
    count = int(known and kind == 1)
    flags = 0x8180 if known else 0x8183
    reply = packet[:2] + struct.pack('!HHHHH', flags, 1, count, 0, 0) + packet[12:end]
    if count:
        reply += b'\xc0\x0c\x00\x01\x00\x01' + struct.pack('!IH', 0, 4) + socket.inet_aton(plan['origin'])
    return reply, {'name': name, 'type': kind, 'transport': transport,
                   'answer': plan['origin'] if count else None, 'ttl': 0,
                   'querySha256': hashlib.sha256(packet).hexdigest()}


class ConfigAdapter:
    """Reuse Network.config_patch/changed_graph, not its loopback transports."""
    def __init__(self, run_id, control=None):
        self.plan = tun.topology(run_id)
        self.domain = base.DOMAIN
        self.health_url = f'http://{self.plan["origin"]}:{self.plan["ports"]["health"]}/health'
        self.control = control
        self.a = SimpleNamespace(address=(self.plan['endpoint']['A'], self.plan['ports']['proxy']))
        self.b = SimpleNamespace(address=(self.plan['endpoint']['B'], self.plan['ports']['proxy']))
        self.dns_a = SimpleNamespace(address=(self.plan['endpoint']['A'], self.plan['ports']['dns']))
        self.dns_b = SimpleNamespace(address=(self.plan['endpoint']['B'], self.plan['ports']['dns']))

    def config_patch(self, mixed_port, control_port, secret):
        patch = base.Network.config_patch(self, mixed_port, control_port, secret)
        patch['proxyModeType'] = 'tun'
        # Keep defaults for dashboard and sandbox; this adapter never emits them.
        for node, role in zip(patch['servers'], ('A', 'B')):
            node['address'] = self.plan['endpoint'][role]
        for resource in patch['dnsServers']:
            resource['endpoint']['host'] = self.plan['endpoint']['B' if resource['id'] == 'loop-b' else 'A']
        a_dns = f'udp://{self.plan["endpoint"]["A"]}:{self.plan["ports"]["dns"]}'
        for key in ('domesticDns', 'foreignDns'):
            patch['dnsConfig'][key] = a_dns
        patch['dnsConfig']['nodeResolverCustom'][0]['spec'] = a_dns
        # Match direct nonloopback destination, not only a DNS-derived loopback.
        for key in ('trafficRules', 'policyRules', 'customRules'):
            patch[key][0]['type'] = 'ipCidr'
            patch[key][0]['values'] = [self.plan['origin'] + '/32']
        patch['dnsRules'][0]['type'] = 'domainSuffix'
        return patch

    changed_graph = staticmethod(base.Network.changed_graph)

    def set_latency(self, label, seconds):
        require(label in ('A', 'B') and type(seconds) in (float, int) and 0 <= seconds <= 2, 'bounded latency')
        require(self.control is not None, 'backend control integration unavailable')
        return self.control(label, {'action': 'latency', 'seconds': seconds})


def direct_connect(plan):
    """No SOCKS handshake, proxy environment or destination substitution."""
    require(os.geteuid() != 0, 'ordinary TUN client UID required')
    require(not any(k.lower() in ('http_proxy', 'https_proxy', 'all_proxy') for k in os.environ), 'proxy environment forbidden')
    host = plan['origin']
    require(not ipaddress.ip_address(host).is_loopback, 'nonloopback TUN destination required')
    return socket.create_connection((host, plan['ports']['echo']), timeout=base.IO_TIMEOUT)


def recv_nonce_frame(conn, stop):
    prefix = bytearray(); deadline = None
    conn.settimeout(0.1)
    while len(prefix) < 2 and not stop.is_set():
        try:
            chunk = conn.recv(2 - len(prefix))
            if not chunk: raise EOFError('origin stream closed')
            if deadline is None: deadline = time.monotonic() + base.IO_TIMEOUT
            prefix.extend(chunk)
        except socket.timeout:
            if deadline is not None and time.monotonic() >= deadline: raise
    require(len(prefix) == 2, 'fixture stopping')
    size = struct.unpack('!H', prefix)[0]
    require(0 < size <= 1024, 'nonce frame bound')
    conn.settimeout(base.IO_TIMEOUT)
    return recv_exact(conn, size)


def exchange(conn, label, nonce):
    require(label in ('A', 'B'), 'expected egress required')
    data = nonce.encode('ascii') if isinstance(nonce, str) else bytes(nonce)
    require(0 < len(data) <= 1024, 'nonce frame bound')
    conn.sendall(struct.pack('!H', len(data)) + data)
    expected = b''.join(label.encode() + bytes([b]) for b in data)
    require(recv_exact(conn, len(expected)) == expected, 'actual egress reply differs')
    return {'exit': label, 'bytes': len(data), 'nonceSha256': hashlib.sha256(data).hexdigest()}


def nonce_dns_query(plan, nonce, transport='udp', destination='8.8.8.8'):
    """Direct UDP/TCP 53 to the production TUN sentinel, not OS DNS proof."""
    require(os.geteuid() != 0 and transport in ('udp', 'tcp'), 'ordinary DNS probe required')
    require(destination == '8.8.8.8', 'production sentinel only')
    require(isinstance(nonce, str) and 1 <= len(nonce) <= 40 and all(c in 'abcdef0123456789' for c in nonce), 'bounded nonce')
    name = nonce + '.' + base.DOMAIN
    wire = b''.join(bytes([len(label)]) + label.encode() for label in name.split('.')) + b'\0'
    ident = os.urandom(2)
    packet = ident + b'\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00' + wire + b'\x00\x01\x00\x01'
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM if transport == 'udp' else socket.SOCK_STREAM)
    try:
        sock.settimeout(base.IO_TIMEOUT)
        sock.connect((destination, 53))
        if transport == 'udp':
            sock.send(packet); answer = sock.recv(4096)
        else:
            sock.sendall(struct.pack('!H', len(packet)) + packet)
            length = struct.unpack('!H', recv_exact(sock, 2))[0]
            require(12 <= length <= 4096, 'DNS answer bound')
            answer = recv_exact(sock, length)
        require(answer[:2] == ident and answer[2:4] == b'\x81\x80' and socket.inet_aton(plan['origin']) in answer,
                'controlled nonce DNS answer differs')
        return {'transport': transport, 'destination': destination, 'name': name, 'type': 1, 'answer': plan['origin'],
                'querySha256': hashlib.sha256(packet).hexdigest(), 'responseSha256': hashlib.sha256(answer).hexdigest()}
    finally:
        sock.close()


def framed_payloads(stream, maximum=1024):
    """Only our two-byte application framing; TCP reassembly belongs to tshark."""
    require(0 < len(stream) <= MAX_BYTES, 'followed stream bound')
    result = []; at = 0
    while at < len(stream):
        require(at + 2 <= len(stream), 'partial application frame header')
        length = struct.unpack_from('!H', stream, at)[0]; at += 2
        require(0 < length <= maximum and at + length <= len(stream), 'partial application frame')
        result.append(stream[at:at + length]); at += length
    return result


def followed_client_bytes(text, destination, port):
    """Decode tshark's documented raw follow output (second direction has tab)."""
    require(isinstance(text, str) and len(text) <= 2 * MAX_BYTES + 65536, 'follow output bound')
    nodes = {}; directions = [bytearray(), bytearray()]
    for line in text.splitlines():
        if not line or line.startswith(('===', 'Follow: tcp,raw', 'Filter:')): continue
        match = re.fullmatch(r'Node ([01]): ([0-9.]+):([0-9]+)', line)
        if match:
            require(int(match[1]) not in nodes, 'duplicate follow endpoint')
            nodes[int(match[1])] = (match[2], int(match[3])); continue
        direction = int(line.startswith('\t')); raw = line.lstrip('\t')
        require(re.fullmatch(r'[0-9a-fA-F]+', raw) and len(raw) % 2 == 0, 'unexpected follow output')
        directions[direction].extend(bytes.fromhex(raw))
    require(set(nodes) == {0, 1}, 'both actual follow endpoints required')
    server = [number for number, endpoint in nodes.items() if endpoint == (destination, port)]
    require(len(server) == 1 and nodes[1 - server[0]][0] != destination, 'direct client/origin follow endpoints differ')
    return bytes(directions[1 - server[0]])


def analyze_capture(path, plan, execute=None, *, supervisor=None):
    """Ordinary-UID offline analysis; requires preinstalled genuine tshark.

No homemade packet/TCP engine. Original pcap hash is supplied alongside mature
follow-stream results. Input must be the completed, root-attested capture copy.
"""
    require(os.geteuid() != 0, 'ordinary offline capture analysis required')
    path = Path(path).absolute()
    require(path.is_file() and not path.is_symlink() and 24 <= path.stat().st_size <= 16 * 1024 * 1024,
            'bounded regular completed capture required')
    require(not (execute is not None and supervisor is not None), 'mock analysis cannot gain real-tool supervision')
    before = hashlib.sha256(path.read_bytes()).hexdigest()
    def real_execute(args):
        tool = tun.root_path(Path('/usr/bin/tshark').resolve())
        env = {'PATH': '/usr/bin:/bin', 'LANG': 'C', 'HOME': os.environ['HOME']}
        if supervisor is None:
            result = subprocess.run([str(tool), *args], stdin=subprocess.DEVNULL, capture_output=True, check=True,
                                    timeout=30, env=env)
            output = result.stdout
        else:
            # The same genuine fixed executable/argv, now registered before any
            # subreaper sample. No caller-controlled real-tool/mocked boolean.
            child = supervisor.spawn([str(tool), *args], env, 'capture-analysis',
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            identity = supervisor.owned[child.pid]
            try:
                output, _ = child.communicate(timeout=30)
            except subprocess.TimeoutExpired:
                supervisor.forced.append({'pid': identity['pid'], 'startTime': identity['startTime'],
                                          'reason': 'capture-analysis-timeout'})
                supervisor.signal(identity, signal.SIGKILL)
                child.wait(timeout=5)
                raise ValueError('bounded real capture analysis timed out') from None
            require(child.returncode == 0, 'real capture analysis failed')
        require(len(output) <= 4 * MAX_BYTES, 'tshark output bound')
        return output.decode('ascii')
    run = execute or real_execute
    common = ['-n', '-r', str(path), '-o', 'tcp.desegment_tcp_streams:TRUE', '-o', 'tcp.analyze_sequence_numbers:TRUE']
    bad = run(common + ['-Y', 'frame.cap_len < frame.len || tcp.analysis.lost_segment || tcp.segment.overlap.conflict',
                         '-T', 'fields', '-e', 'frame.number']).strip()
    require(not bad, 'capture truncated/lost/conflicting segments')
    nonces = []; dns_queries = []; packets = 0
    for destination, port, mode in ((plan['origin'], plan['ports']['echo'], 'echo'), ('8.8.8.8', 53, 'dns')):
        selector = f'ip.dst == {destination} && tcp.dstport == {port}'
        indices = run(common + ['-Y', selector, '-T', 'fields', '-e', 'tcp.stream']).split()
        require(len(indices) <= 4096 and all(v.isdigit() and int(v) < 4096 for v in indices), 'bounded capture stream IDs')
        require(len(set(indices)) <= 64, 'bounded stream count')
        packets += len(indices)
        for index in sorted(set(indices), key=int):
            syn = run(common + ['-Y', selector + f' && tcp.stream == {index} && tcp.flags.syn == 1 && tcp.flags.ack == 0',
                                '-T', 'fields', '-e', 'frame.number']).strip()
            require(syn, 'capture must include client SYN')
            followed = run(common + ['-q', '-z', 'follow,tcp,raw,' + index])
            stream = followed_client_bytes(followed, destination, port)
            for payload in framed_payloads(stream, 4096 if mode == 'dns' else 1024):
                if mode == 'echo':
                    nonces.append({'stream': int(index), 'nonceSha256': hashlib.sha256(payload).hexdigest(), 'bytes': len(payload)})
                else:
                    name, kind, _ = base.dns_question(payload)
                    dns_queries.append({'transport': 'tcp', 'destination': destination, 'name': name, 'type': kind,
                                        'querySha256': hashlib.sha256(payload).hexdigest()})
    udp = run(common + ['-Y', 'ip.dst == 8.8.8.8 && udp.dstport == 53 && dns.flags.response == 0',
                       '-T', 'fields', '-e', 'udp.payload']).splitlines()
    require(len(udp) <= 4096, 'UDP query count bound')
    for raw in udp:
        raw = raw.replace(':', '')
        require(re.fullmatch(r'[0-9a-fA-F]+', raw) and len(raw) <= 8192, 'bounded UDP DNS payload')
        payload = bytes.fromhex(raw); name, kind, _ = base.dns_question(payload)
        dns_queries.append({'transport': 'udp', 'destination': '8.8.8.8', 'name': name, 'type': kind,
                            'querySha256': hashlib.sha256(payload).hexdigest()})
    require(nonces and hashlib.sha256(path.read_bytes()).hexdigest() == before, 'no TUN nonces or capture changed')
    return {'captureSha256': before, 'analysis': 'tshark-follow-tcp-raw',
            'destination': plan['origin'], 'nonces': nonces, 'dnsQueries': dns_queries, 'packets': packets,
            'analysisExecution': 'simulated' if execute else 'real-tool'}


class Server:
    def __init__(self, plan, role, expected_netns):
        require(os.geteuid() != 0 and role in ('A', 'B', 'O'), 'ordinary fixture role required')
        require(Path('/proc/self/ns/net').stat().st_ino == expected_netns and
                expected_netns != Path('/proc/1/ns/net').stat().st_ino, 'registered backend namespace required')
        self.plan, self.role = plan, role
        self.stop = threading.Event(); self.lock = threading.Lock()
        self.sockets = set(); self.threads = []; self.rows = []; self.errors = []; self.listeners = []
        self.latency = 0.15 if role == 'B' else 0.0
        self.total = 0
        try:
            if role == 'O':
                for name in ('echo', 'health'):
                    self.listen(plan['origin'], plan['ports'][name], name)
            else:
                self.listen(plan['endpoint'][role], plan['ports']['proxy'], 'proxy')
                self.listen(plan['endpoint'][role], plan['ports']['dns'], 'dns-tcp')
                self.listen_udp(plan['endpoint'][role], plan['ports']['dns'])
                if role == 'A': self.listen(plan['special'], plan['ports']['special'], 'special')
        except BaseException:
            self.close(); raise

    def listen(self, host, port, kind):
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listeners.append(sock)
        sock.bind((host, port)); sock.listen(32); sock.settimeout(0.1)
        thread = threading.Thread(target=self.accept, args=(sock, kind), daemon=False)
        self.threads.append(thread); thread.start()

    def accept(self, listener, kind):
        while not self.stop.is_set():
            try: conn, peer = listener.accept()
            except socket.timeout: continue
            except OSError:
                if not self.stop.is_set(): self.errors.append('accept-failed')
                return
            with self.lock:
                self.total += 1
                allowed = len(self.sockets) < 128 and self.total <= 2048
                if allowed: self.sockets.add(conn)
            if not allowed:
                conn.close(); self.errors.append('connection-bound'); continue
            conn.settimeout(base.IO_TIMEOUT)
            thread = threading.Thread(target=self.handle, args=(conn, peer, kind), daemon=False)
            self.threads.append(thread); thread.start()

    def listen_udp(self, host, port):
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.listeners.append(sock); sock.bind((host, port)); sock.settimeout(0.1)
        def receive():
            while not self.stop.is_set():
                try:
                    packet, peer = sock.recvfrom(4096)
                    require(peer[0] == self.plan['host'][self.role], 'DNS peer outside owned veth')
                    answer, row = dns_response(self.plan, packet, 'udp')
                    with self.lock:
                        require(len(self.rows) < 4096, 'bounded observations')
                        self.rows.append(dict(row, role=self.role))
                    sock.sendto(answer, peer)
                except socket.timeout: pass
                except OSError:
                    if not self.stop.is_set(): self.errors.append('udp-failed')
                    return
                except (ValueError, UnicodeError): self.errors.append('dns-rejected')
        thread = threading.Thread(target=receive, daemon=False)
        self.threads.append(thread); thread.start()

    def handle(self, conn, peer, kind):
        outgoing = None
        try:
            if kind == 'proxy':
                require(peer[0] == self.plan['host'][self.role], 'proxy peer outside owned veth')
                host, port = allowed_target(self.plan, *parse_socks_request(conn))
                self.stop.wait(self.latency)
                require(not self.stop.is_set(), 'fixture stopping')
                outgoing = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                outgoing.settimeout(base.IO_TIMEOUT)
                outgoing.bind((self.plan['exit'][self.role], 0)); outgoing.connect((host, port))
                with self.lock: self.sockets.add(outgoing)
                conn.sendall(b'\x05\x00\x00\x01' + socket.inet_aton(self.plan['exit'][self.role]) + b'\x00\x00')
                row = {'role': self.role, 'kind': 'proxy', 'destination': host, 'port': port,
                       'source': outgoing.getsockname()[0], 'bytesIn': 0, 'bytesOut': 0}
                with self.lock: self.rows.append(row)
                with selectors.DefaultSelector() as selected:
                    selected.register(conn, selectors.EVENT_READ, (outgoing, 'bytesIn'))
                    selected.register(outgoing, selectors.EVENT_READ, (conn, 'bytesOut'))
                    while not self.stop.is_set():
                        for key, _ in selected.select(0.1):
                            data = key.fileobj.recv(4096)
                            if not data: return
                            target, counter = key.data
                            require(row[counter] + len(data) <= MAX_BYTES, 'relay byte bound')
                            target.sendall(data)
                            with self.lock: row[counter] += len(data)
            elif kind == 'dns-tcp':
                require(peer[0] == self.plan['host'][self.role], 'DNS peer outside owned veth')
                length = struct.unpack('!H', recv_exact(conn, 2))[0]
                require(12 <= length <= 4096, 'DNS query bound')
                answer, row = dns_response(self.plan, recv_exact(conn, length), 'tcp')
                with self.lock: self.rows.append(dict(row, role=self.role))
                conn.sendall(struct.pack('!H', len(answer)) + answer)
            elif kind == 'special':
                require(peer[0] == self.plan['host']['A'], 'special route peer differs')
                conn.sendall(b'synthetic-route-only\n')
            else:
                labels = {address: label for label, address in self.plan['exit'].items()}
                require(peer[0] in labels, 'origin requires actual A/B exit address')
                label = labels[peer[0]]
                conn.settimeout(0.1)
                consumed = 0
                while not self.stop.is_set():
                    if kind == 'health':
                        try: data = conn.recv(4096)
                        except socket.timeout: continue
                        if not data: return
                        require(data.startswith((b'GET /health ', b'HEAD /health ')), 'health path differs')
                        conn.sendall(b'HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n'); return
                    # One bounded length-prefixed nonce, independent of TCP segmentation.
                    data = recv_nonce_frame(conn, self.stop)
                    consumed += len(data)
                    require(consumed <= MAX_BYTES, 'origin byte bound')
                    with self.lock:
                        require(len(self.rows) < 4096, 'origin observations bound')
                        self.rows.append({'role': 'O', 'kind': kind, 'originPeer': peer[0], 'exit': label,
                                          'bytes': len(data), 'nonceSha256': hashlib.sha256(data).hexdigest()})
                    conn.sendall(b''.join(label.encode() + bytes([b]) for b in data))
        except (EOFError, ConnectionError, socket.timeout): pass
        except (ValueError, UnicodeError, OSError):
            if not self.stop.is_set(): self.errors.append('fixture-protocol-or-socket-failed')
        finally:
            for sock in (conn, outgoing):
                if sock is not None:
                    with self.lock: self.sockets.discard(sock)
                    sock.close()

    def snapshot(self):
        with self.lock:
            return {'role': self.role, 'rows': copy.deepcopy(self.rows), 'errors': list(self.errors),
                    'latency': self.latency, 'netns': Path('/proc/self/ns/net').stat().st_ino}

    def close(self):
        self.stop.set()
        for sock in self.listeners:
            sock.close()
        with self.lock: owned = list(self.sockets)
        for sock in owned:
            try: sock.shutdown(socket.SHUT_RDWR)
            except OSError: pass
        # A finite monotonic budget, not N threads * timeout.
        end = time.monotonic() + 5
        for thread in list(self.threads): thread.join(max(0, end - time.monotonic()))
        require(not any(thread.is_alive() for thread in self.threads), 'fixture threads not closed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run-id', required=True); parser.add_argument('--role', choices=('A', 'B', 'O'), required=True)
    parser.add_argument('--namespace-inode', type=int, required=True)
    args = parser.parse_args()
    server = Server(tun.topology(args.run_id), args.role, args.namespace_inode)
    try:
        print(json.dumps({'ready': True, 'role': args.role}), flush=True)
        while line := sys.stdin.buffer.readline(4097):
            require(len(line) <= 4096 and line.endswith(b'\n'), 'bounded private control request')
            request = json.loads(line)
            if request == {'action': 'stop'}: break
            if request.get('action') == 'latency':
                seconds = request.get('seconds')
                require(args.role in ('A', 'B') and type(seconds) in (int, float) and 0 <= seconds <= 2, 'bounded latency')
                server.latency = seconds
            else: require(request == {'action': 'snapshot'}, 'unknown private control command')
            print(json.dumps(server.snapshot()), flush=True)
    finally:
        server.close()
        print(json.dumps({'closed': True, **server.snapshot()}), flush=True)


if __name__ == '__main__': main()
