#!/usr/bin/env python3
"""Prepare private Darwin C4 candidates from the exact shipped sing-box tree."""
import argparse
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
HEAD = 'b609f959f57ce34416c51c7b87ce4a76f2e1df56'
QUIC = Path('/home/sway/go/pkg/mod/github.com/sagernet/quic-go@v0.61.0-sing-box-mod.7')
SING_QUIC = Path('/home/sway/go/pkg/mod/github.com/sagernet/sing-quic@v0.7.1-0.20260924092235-4f371c86a365')
PINS = {
    'go.mod': '44f8313894387dd4576ec8839ab2bb74307748e378d79863be778f05abdeaa5c',
    'go.sum': 'b0b62e1b51d268a1c569bf0ec39b72debb9683036d4913edfd8dee073db3a64f',
    'transport/http/client_h3.go': '56f9b14746770c88793e45f0f8afd8b3dcddb7261cca1a9546caabe9f191e1a2',
    'common/tls/client.go': 'c251099384a473024e96e81efba8a8ab60fd8666f50e2ab299d75f7f2aaa1030',
    'common/tls/std_client.go': '13810a256e9dddaef91aa5dee9d46603d033ff8f2dbf6bd773b67ffdcb290fa9',
    'common/dialer/default.go': 'ac6fd0e233b0bb5f6a44aa3feb464dbbb94d1ad6dd1736fc871d9787d3efbb9b',
    'route/conn.go': '1bb6cca812396b8663069e1840ff4bb3423be7c1920c316e666b18162da1f049',
    'cmd/sing-box/cmd_run.go': 'f693f3714319acccf37bc5c1917308e69496259610271c508f007397ead4c6a5',
    'release/LDFLAGS': 'd0a3a04eb9ac5f3018978515e4864ba61cca69d9a99101d11d7970e537a1991d',
    'release/DEFAULT_BUILD_TAGS': '82e10e608ca6e958c9d7fe1f4a523b151e6c25407682264e2434a081e108befa',
}
LINK_FLAGS = '-X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -checklinkname=0'
QUIC_PINS = {
    'go.mod': '34b1c21ccffa98125b3f6c334d166f83a0820c8cc4bc88343ee4495fabd0ce0c',
    'sys_conn.go': 'a3066d6c45d54f28429185f1a2e394c22d3576b9feab1812ab715972fe3fb6fb',
    'sys_conn_oob.go': '2a13164b872a74303c8245ea77aadde10243adecd505f5d3ec2dd74edaf2efc8',
    'sys_conn_msgx_darwin.go': 'b9088adc5e9b1465496c56ca119dd48c5d60a01fefd1e9109d1cd83bc9ad4805',
    'sys_conn_msgx_macos.go': '41b7b98128ad199b691d582f4a1356cc69ee396c5d5dbf90a12c866d0142f88f',
    'client.go': '4b14d86104383eada582022596fff56256e26b67b19196c5bc4f39bf679c975e',
    'transport.go': 'ba7eea131dc6687767f0da6e890078b26cf050e0dda3788ba1dcda6c308840cd',
}
SING_QUIC_PIN = '35a8d7f23e7617a40374ed8d9491728b93d67f090b5e3abd8a3aedeacb757e7a'
SING_QUIC_TREE_SHA256 = 'cfd7ad37686559a1e401066333194e3a615172c2509855f8cbae171484a2deac'
SING_QUIC_GO_SUM = 'h1:WW+kS6rXI6fGOGDBYnaVPr2q68FXjP/vhw5pyXmbRQM='
# Entire extracted module tree: 253 entries / 227 regular files. The cached
# module ZIP was checked byte-for-byte against extraction, and offline
# `go mod download -json` returned the go.sum h1 below without network access.
# The digest algorithm is tree_sha(), not a runtime self-derived expectation.
QUIC_TREE_SHA256 = 'f191d9a78ee3c32fd063e5c374f4dabeed8ca919b7b9d4fe00321959e8f02fcc'
QUIC_GO_SUM = 'h1:sW2O+DoNF+my1PGlSZ3zG2N+fdxLR3DC0l4sXCM0lBE='
TAGS = ('badlinkname,tfogo_checklinkname0,with_acme,with_ccm,with_clash_api,'
        'with_cloudflared,with_dhcp,with_gvisor,with_naive_outbound,with_ocm,'
        'with_openconnect,with_openvpn,with_quic,with_tailscale,with_usbip,'
        'with_utls,with_wireguard')
BUILD_PROFILE = 'darwin-arm64-cgo1-proc-only-v2'
SOCKET_STATE_SOURCES = ('polaris_socket_state_darwin_cgo.go',
                        'polaris_socket_state_stub.go')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def link_flags(source):
    data = (source / 'release/LDFLAGS').read_bytes()
    if sha(data) != PINS['release/LDFLAGS'] or data != (LINK_FLAGS + '\n').encode():
        raise RuntimeError('fixed release/LDFLAGS source differs')
    return LINK_FLAGS


def official_tags(source):
    data = (source / 'release/DEFAULT_BUILD_TAGS').read_bytes()
    if sha(data) != PINS['release/DEFAULT_BUILD_TAGS']:
        raise RuntimeError('fixed release/DEFAULT_BUILD_TAGS source differs')
    actual = data.decode('ascii').strip().split(',')
    configured = TAGS.split(',')
    if len(actual) != len(set(actual)) or set(actual) != set(configured):
        raise RuntimeError('candidate build tags differ from fixed release default')
    return TAGS


def compiled_source_root(source, build):
    """Go resolves the /var/tmp symlink on macOS before matching overlays."""
    canonical = source.resolve(strict=True)
    if build:
        expected = Path('/private') / source.relative_to('/')
        if canonical != expected:
            raise RuntimeError('Darwin source physical path differs from /private/var/tmp contract')
    return canonical


def socket_state_abi():
    """Compile and run a public-SDK-only ABI witness on the native CI Mac."""
    def output(argv):
        return subprocess.run(argv, check=True, capture_output=True, text=True,
                              timeout=20).stdout.strip()
    sdk_path = output(['xcrun', '--sdk', 'macosx', '--show-sdk-path'])
    sdk_version = output(['xcrun', '--sdk', 'macosx', '--show-sdk-version'])
    if not Path(sdk_path).is_absolute() or not re.fullmatch(r'[0-9]+(?:\.[0-9]+)*', sdk_version):
        raise RuntimeError('native macOS SDK identity unavailable')
    code = (b'#include <stdio.h>\n#include <sys/proc_info.h>\n'
            b'_Static_assert(PROC_PIDFDSOCKETINFO == 3, "flavor");\n'
            b'_Static_assert(SOI_S_CANTSENDMORE == 0x0010, "state bit");\n'
            b'_Static_assert(PROC_PIDFDSOCKETINFO_SIZE == sizeof(struct socket_fdinfo), "size");\n'
            b'int main(void) { printf("%zu %zu %d %u\\n", '
            b'sizeof(struct socket_fdinfo), (size_t)PROC_PIDFDSOCKETINFO_SIZE, '
            b'PROC_PIDFDSOCKETINFO, (unsigned)SOI_S_CANTSENDMORE); return 0; }\n')
    with tempfile.TemporaryDirectory(prefix='c4-public-socket-abi-') as folder:
        binary = Path(folder) / 'abi'
        subprocess.run(['xcrun', '--sdk', 'macosx', 'clang', '-isysroot', sdk_path,
                        '-x', 'c', '-', '-o', str(binary)], input=code, check=True,
                       capture_output=True, timeout=30)
        parts = output([str(binary)]).split()
    if len(parts) != 4 or not all(part.isdecimal() for part in parts):
        raise RuntimeError('public socket ABI witness output invalid')
    size, macro_size, flavor, state_bit = map(int, parts)
    if not (0 < size <= 4096 and size == macro_size and flavor == 3 and state_bit == 16):
        raise RuntimeError('public socket ABI witness differs')
    return {'sdkPath': sdk_path, 'sdkVersion': sdk_version,
            'socketFdinfoSize': size, 'macroSize': macro_size,
            'flavor': flavor, 'cannotSendBit': state_bit}


def compile_witness(binary, mode):
    """Prove every required overlaid call site survived compilation and linking."""
    if mode not in ('raw', 'dialudp'):
        raise ValueError('unknown candidate mode')
    symbols = {
        'h3': 'github.com/sagernet/sing-box/transport/http.(*http3ClientImpl).acquire',
        'socketOwner': 'github.com/sagernet/sing-box/route.(*socketOwner).close',
        'trackedDetach': 'github.com/sagernet/sing-box/route.(*trackedConn).Detach',
        'trackedClose': 'github.com/sagernet/sing-box/route.(*trackedConn).Close',
        'runFinalize': 'main.run.func1',
        'failureProbe': 'github.com/sagernet/quic-go.PolarisTraceDialFailureProbe',
        'failureProbeControl': 'github.com/sagernet/quic-go.PolarisTraceDialFailureProbe.func1',
        'stateWrapper': 'github.com/sagernet/quic-go.polarisReadSelfSocketState',
    }
    calls = {}
    for site, symbol in symbols.items():
        output = subprocess.run(
            ['go', 'tool', 'objdump', '-s', '^' + re.escape(symbol) + '$',
             str(binary)], check=True, capture_output=True, text=True,
            timeout=30).stdout
        if f'TEXT {symbol}(SB)' not in output:
            raise RuntimeError(f'candidate compile witness missing site: {site}')
        calls[site] = [name.removesuffix('.abi0') for name in
                       re.findall(r'\bCALL ([^\s]+)\(SB\)', output)]
    def count(site, symbol):
        return calls[site].count(symbol)
    trace = 'github.com/sagernet/quic-go.'
    # Every target is fixed. Report bounded counts only, never objdump bytes,
    # paths or arbitrary symbols. The Control callback is its own Go symbol.
    rules = (
        ('h3.traceDial', 'h3', trace + 'PolarisTraceDial', 1),
        ('h3.failureProbe', 'h3', trace + 'PolarisTraceDialFailureProbe', 1),
        ('h3.dialEarly', 'h3', 'github.com/sagernet/sing-quic.DialEarly', 1),
        ('h3.dialUDP', 'h3', 'net.dialUDP', int(mode == 'dialudp')),
        ('owner.close', 'socketOwner', trace + 'PolarisTraceObservedClose', 1),
        ('detach.close', 'trackedDetach', trace + 'PolarisTraceObservedClose', 1),
        ('tracked.close', 'trackedClose', trace + 'PolarisTraceObservedClose', 1),
        ('run.finalize', 'runFinalize', trace + 'PolarisTraceFinalize', 1),
        ('control.proc', 'failureProbeControl',
         trace + 'polarisReadSelfSocketState', 1),
        ('wrapper.cgo', 'stateWrapper',
         trace + '_Cfunc_polaris_self_socket_state', 1),
    )
    measured = [(label, expected, count(site, target))
                for label, site, target, expected in rules]
    if any(actual != expected for _, expected, actual in measured):
        vector = ','.join(f'{label}:{expected}/{actual if actual < 2 else "2+"}'
                          for label, expected, actual in measured)
        raise RuntimeError('candidate compiled call sites differ [' + vector + ']')
    return {
        'h3SocketCall': 'net.dialUDP' if mode == 'dialudp' else 'RawDialer',
        'h3TraceDial': True, 'h3SocketProbe': True, 'socketOwnerClose': True,
        'trackedDetachClose': True, 'trackedClose': True,
        'gracefulFinalize': True,
        'procSelfSocketState': True,
    }


def tree_sha(root):
    if not root.is_dir() or root.is_symlink():
        raise RuntimeError('quic-go reference tree root invalid')
    h = hashlib.sha256()
    for path in sorted(root.rglob('*'), key=lambda x: str(x.relative_to(root))):
        mode = path.lstat().st_mode
        kind = b'F' if stat.S_ISREG(mode) else b'D' if stat.S_ISDIR(mode) else None
        if kind is None:
            raise RuntimeError('quic-go reference tree contains symlink/special file')
        h.update(kind + b'\0' + str(path.relative_to(root)).encode() + b'\0')
        if kind == b'F':
            h.update(hashlib.sha256(path.read_bytes()).digest())
        h.update(b'\n')
    return h.hexdigest()


def dial_failure_ownership_gate(source, quic_reference, sing_quic_reference):
    """Prove the pinned standard-TLS error path stops QUIC but retains rawConn.

    This is a source contract for the fixture's default Go TLS engine. The
    runtime socket liveness assertion is in test_local.py.
    """
    sing = (sing_quic_reference / 'quic.go').read_text()
    client = (quic_reference / 'client.go').read_text()
    transport = (quic_reference / 'transport.go').read_text()
    tls = (source / 'common/tls/client.go').read_text()
    std_tls = (source / 'common/tls/std_client.go').read_text()
    dial = client.split('func DialEarlyConn(', 1)[1].split('\nfunc setupTransportConn(', 1)[0]
    setup = client.split('func setupTransportConn(', 1)[1].split('\nfunc setupTransport(', 1)[0]
    close = transport.split('func (t *Transport) Close() error {', 1)[1].split(
        '\nfunc (t *Transport) closeServer()', 1)[0]
    if not (sing.count('quic.DialEarlyConn(ctx, conn, tlsConfig, quicConfig)') == 1 and
            'conn = withSyscallConn(conn)' in sing and
            'conn, err := dl.DialEarly(ctx, c.RemoteAddr(), tlsConf, conf)' in dial and
            'if err != nil {\n\t\tdl.Close()\n\t\treturn nil, err\n\t}' in dial and
            'Conn:        conn.(net.PacketConn)' in setup and
            'isSingleUse: true' in setup and 'createdConn:' not in setup and
            'if t.createdConn {\n\t\tif err := t.Conn.Close()' in close and
            '} else if t.conn != nil {\n\t\tt.conn.SetReadDeadline(time.Now())' in close and
            close.count('t.Conn.Close()') == 1 and
            'case "", C.TLSEngineGo:' in tls and
            'return newSTDClient(' in tls and
            'func (c *STDClientConfig) STDConfig()' in std_tls):
        raise RuntimeError('pinned DialEarly failure rawConn ownership contract changed')


def trace_probe_guard_gate(source):
    """Reject a probe that can query another FD generation or a closing FD."""
    anchor = 'controlErr := raw.Control(func(fd uintptr) {'
    if source.count(anchor) != 1:
        raise RuntimeError('socket probe Control anchor changed')
    callback = source.split(anchor, 1)[1].split('\n\t\t\t})', 1)[0]
    checks = ('active := polarisByFD[id.FD] == id',
              'int(fd) != id.FD || !active || id.firstClose.Load()',
              'queryErr = syscall.EBADF',
              'return',
              'syscall.Getsockname(int(fd))',
              'syscall.Getpeername(int(fd))',
              'polarisReadSelfSocketState(int(fd))',
              'syscall.GetsockoptInt(int(fd), syscall.SOL_SOCKET, syscall.SO_ERROR)')
    positions = [callback.find(item) for item in checks]
    if any(pos < 0 for pos in positions) or positions != sorted(positions):
        raise RuntimeError('socket probe FD/generation/Close guard is not first')
    guard = checks[1]
    guards = [match.start() for match in re.finditer(re.escape(guard), callback)]
    if (len(guards) != 3 or guards[1] <= positions[5] or
            guards[1] >= positions[6] or guards[2] <= positions[7] or
            'stateDetail, stateErr = "identity_guard_errno", syscall.EBADF' not in
            callback[guards[2]:]):
        raise RuntimeError('socket probe FD/generation/Close guard missing around proc/SO_ERROR')


def check_source(source, quic_reference=QUIC, sing_quic_reference=SING_QUIC):
    if not source.is_dir() or source.is_symlink():
        raise RuntimeError('source worktree missing or symlinked')
    result = subprocess.run(['git', '-C', str(source), 'rev-parse', 'HEAD'],
                            check=True, capture_output=True, text=True)
    if result.stdout.strip() != HEAD:
        raise RuntimeError('source worktree is not the exact shipped b609 commit')
    result = subprocess.run(['git', '-C', str(source), 'status', '--porcelain'],
                            check=True, capture_output=True, text=True)
    if result.stdout.strip():
        raise RuntimeError('source worktree is dirty')
    subprocess.run(['git', '-C', str(source), 'fsck', '--strict', '--full',
                    '--no-reflogs'], check=True, capture_output=True,
                   text=True, timeout=60)
    for name, expected in PINS.items():
        if sha((source / name).read_bytes()) != expected:
            raise RuntimeError(f'sing-box source pin mismatch: {name}')
    link_flags(source)
    official_tags(source)
    for name, expected in QUIC_PINS.items():
        if sha((quic_reference / name).read_bytes()) != expected:
            raise RuntimeError(f'quic-go source pin mismatch: {name}')
    if tree_sha(quic_reference) != QUIC_TREE_SHA256:
        raise RuntimeError('quic-go complete reference tree pin mismatch')
    if sha((sing_quic_reference / 'quic.go').read_bytes()) != SING_QUIC_PIN:
        raise RuntimeError('sing-quic withSyscallConn source pin mismatch')
    if tree_sha(sing_quic_reference) != SING_QUIC_TREE_SHA256:
        raise RuntimeError('sing-quic complete reference tree pin mismatch')
    dial_failure_ownership_gate(source, quic_reference, sing_quic_reference)
    if ('github.com/sagernet/quic-go v0.61.0-sing-box-mod.7 ' + QUIC_GO_SUM) not in \
            (source / 'go.sum').read_text().splitlines():
        raise RuntimeError('quic-go module h1 differs from pinned go.sum')
    if ('github.com/sagernet/sing-quic v0.7.1-0.20260924092235-4f371c86a365 ' +
            SING_QUIC_GO_SUM) not in (source / 'go.sum').read_text().splitlines():
        raise RuntimeError('sing-quic module h1 differs from pinned go.sum')
    gomod = (source / 'go.mod').read_text()
    for line in ('github.com/sagernet/quic-go v0.61.0-sing-box-mod.7',
                 'github.com/sagernet/sing-quic v0.7.1-0.20260924092235-4f371c86a365'):
        if line not in gomod:
            raise RuntimeError(f'shipped dependency pin mismatch: {line}')


def replace_one(data, old, new, label):
    if data.count(old) != 1:
        raise RuntimeError(f'private patch anchor changed: {label} ({data.count(old)})')
    return data.replace(old, new)


def patch_h3(source, mode):
    data = source.read_text()
    data = replace_one(data, '\trawConn       net.Conn\n',
                       '\trawConn       net.Conn\n\ttraceIdentity *quic.PolarisTraceIdentity\n',
                       'H3 trace identity fields')
    old = '''rawConn, err := c.dialer.DialContext(ctx, N.NetworkUDP, c.server)
\tif err != nil {
\t\tif ctx.Err() != nil {
\t\t\treturn nil, ctx.Err()
\t\t}
\t\treturn nil, E.Cause1(ErrHTTP3Unavailable, err)
\t}
\tquicConn, err := qtls.DialEarly(ctx, rawConn, c.tlsConfig, c.quicConfig)'''
    dial = ('c.dialer.DialContext(ctx, N.NetworkUDP, c.server)' if mode == 'raw'
            else 'net.DialUDP("udp4", nil, c.server.UDPAddr())')
    new = f'''// C4 private candidate: only the socket creation call differs by build.
\trawConn, err := {dial}
\tif err != nil {{
\t\tif ctx.Err() != nil {{
\t\t\treturn nil, ctx.Err()
\t\t}}
\t\treturn nil, E.Cause1(ErrHTTP3Unavailable, err)
\t}}
\ttraceIdentity, traceErr := quic.PolarisTraceDial(rawConn, "{mode}")
\tif traceErr != nil {{
\t\trawConn.Close()
\t\treturn nil, E.Cause1(ErrHTTP3Unavailable, traceErr)
\t}}
\tquicConn, err := qtls.DialEarly(ctx, rawConn, c.tlsConfig, c.quicConfig)'''
    data = replace_one(data, old, new, 'H3 acquire')
    data = replace_one(data, '\t\trawConn.Close()\n\t\tif ctx.Err() != nil {',
                       '\t\tquic.PolarisTraceDialFailureProbe(rawConn, traceIdentity)\n'
                       '\t\tquic.PolarisTraceClose(rawConn, traceIdentity, "dial_failure")\n'
                       '\t\tif ctx.Err() != nil {', 'H3 dial failure close')
    data = replace_one(data, '\tc.rawConn = rawConn\n',
                       '\tc.rawConn = rawConn\n\tc.traceIdentity = traceIdentity\n',
                       'H3 stored trace identity')
    if data.count('\t\tc.rawConn.Close()\n\t\tc.rawConn = nil\n') != 3:
        raise RuntimeError('H3 close sites changed')
    data = data.replace('\t\tc.rawConn.Close()\n\t\tc.rawConn = nil\n',
                        '\t\tquic.PolarisTraceClose(c.rawConn, c.traceIdentity, "http3_release")\n'
                        '\t\tc.rawConn = nil\n\t\tc.traceIdentity = nil\n')
    return data


def patch_oob(source):
    data = source.read_text()
    data = replace_one(data,
                       'func (c *connectedCompatConn) SetLinger(int) error { return nil }\n',
                       'func (c *connectedCompatConn) SetLinger(int) error { return nil }\n\n'
                       'func (c *connectedCompatConn) Close() error {\n'
                       '\tif c.polaris == nil { return c.Conn.Close() }\n'
                       '\treturn polarisTransportClose(c.Conn, c.polaris)\n'
                       '}\n', 'transport close')
    data = replace_one(data, 'type connectedCompatConn struct {\n\tnet.Conn\n',
                       'type connectedCompatConn struct {\n\tnet.Conn\n'
                       '\tpolaris *PolarisTraceIdentity\n', 'connected identity')
    data = replace_one(data, 'type oobConn struct {\n\tnet.PacketConn\n',
                       'type oobConn struct {\n\tnet.PacketConn\n'
                       '\tpolaris *PolarisTraceIdentity\n', 'OOB identity')
    data = replace_one(data,
                       '\tpacketConn := &connectedCompatConn{Conn: c, sysConn: sysConn}\n',
                       '\tpolaris := polarisBridgeRaw(sysConn)\n'
                       '\tpacketConn := &connectedCompatConn{Conn: c, sysConn: sysConn, polaris: polaris}\n',
                       'one-time connected bridge')
    data = replace_one(data,
                       '\t\tbc = ipv4.NewPacketConn(packetConn)\n\t}\n\n'
                       '\tmsgs := make([]ipv4.Message, batchSize)\n',
                       '\t\tbc = ipv4.NewPacketConn(packetConn)\n\t}\n\n'
                       '\tif reader, ok := bc.(*msgXReader); ok { reader.polaris = polaris }\n'
                       '\tmsgs := make([]ipv4.Message, batchSize)\n',
                       'reader identity')
    data = replace_one(data,
                       '\t\tPacketConn:     packetConn,\n',
                       '\t\tPacketConn:     packetConn,\n\t\tpolaris:        polaris,\n',
                       'OOB instance identity')
    old = '''\tif gso {
\t\toobConn.segmentWriter = newSegmentWriter(sysConn)
\t}
\tif activityConn, ok := c.(IOActivityConn); ok {'''
    new = '''\tif gso {
\t\toobConn.segmentWriter = newSegmentWriter(sysConn)
\t\tif writer, ok := oobConn.segmentWriter.(*msgXWriter); ok { writer.polaris = polaris }
\t}
\tif polaris != nil {
\t\treader := "fallback"
\t\tif _, ok := bc.(*msgXReader); ok { reader = "msgx" }
\t\twriter := "sendmsg_only"
\t\tif gso { writer = "gso_instrumented" }
\t\tpolarisEvent(polaris, "reader_selected", 0, nil, reader)
\t\tpolarisEvent(polaris, "writer_selected", 0, nil, writer)
\t}
\tif activityConn, ok := c.(IOActivityConn); ok {'''
    data = replace_one(data, old, new, 'connected path selection')
    old = '''\t\t\tn, sendErr = unix.SendmsgN(int(fd), b, oob, sockaddr, 0)
\t\t\tif sendErr == unix.EINTR {'''
    new = '''\t\t\tif c.polaris != nil { polarisEvent(c.polaris, "send_enter", len(b), nil, "sendmsg") }
\t\t\tn, sendErr = unix.SendmsgN(int(fd), b, oob, sockaddr, 0)
\t\t\tif c.polaris != nil { polarisEvent(c.polaris, "send_exit", n, sendErr, "sendmsg") }
\t\t\tif sendErr == unix.EINTR {'''
    data = replace_one(data, old, new, 'sendmsg')
    return data


def patch_msgx(source):
    data = source.read_text()
    data = replace_one(data, 'type msgXReader struct {\n\trawConn   syscall.RawConn\n',
                       'type msgXReader struct {\n\tpolaris *PolarisTraceIdentity\n'
                       '\trawConn   syscall.RawConn\n', 'msgXReader identity')
    data = replace_one(data, 'type msgXWriter struct {\n\trawConn syscall.RawConn\n',
                       'type msgXWriter struct {\n\tpolaris *PolarisTraceIdentity\n'
                       '\trawConn syscall.RawConn\n', 'msgXWriter identity')
    old = '''\t\t\t//nolint:staticcheck
\t\t\tn, _, errno = unix.RawSyscall6(unix.SYS_RECVMSG_X, fd,
\t\t\t\tuintptr(unsafe.Pointer(&r.hdrs[0])), uintptr(count), unix.MSG_DONTWAIT, 0, 0)
\t\t\tif errno == unix.EINTR {'''
    new = '''\t\t\tif r.polaris != nil { polarisEvent(r.polaris, "recv_enter", count, nil, "recvmsg_x") }
\t\t\t//nolint:staticcheck
\t\t\tn, _, errno = unix.RawSyscall6(unix.SYS_RECVMSG_X, fd,
\t\t\t\tuintptr(unsafe.Pointer(&r.hdrs[0])), uintptr(count), unix.MSG_DONTWAIT, 0, 0)
\t\t\tif r.polaris != nil { polarisEvent(r.polaris, "recv_exit", int(n), errno, "recvmsg_x") }
\t\t\tif errno == unix.EINTR {'''
    data = replace_one(data, old, new, 'recvmsg_x')
    old = '''\t\t\t//nolint:staticcheck
\t\t\tn, _, errno := unix.RawSyscall6(unix.SYS_SENDMSG_X, fd,
\t\t\t\tuintptr(unsafe.Pointer(&hdrs[sent])), uintptr(count-sent), 0, 0, 0)
\t\t\tswitch {'''
    new = '''\t\t\tif w.polaris != nil { polarisEvent(w.polaris, "send_enter", count-sent, nil, "sendmsg_x") }
\t\t\t//nolint:staticcheck
\t\t\tn, _, errno := unix.RawSyscall6(unix.SYS_SENDMSG_X, fd,
\t\t\t\tuintptr(unsafe.Pointer(&hdrs[sent])), uintptr(count-sent), 0, 0, 0)
\t\t\tif w.polaris != nil { polarisEvent(w.polaris, "send_exit", int(n), errno, "sendmsg_x") }
\t\t\tswitch {'''
    return replace_one(data, old, new, 'sendmsg_x')


def patch_route(source):
    data = source.read_text()
    data = replace_one(data, '\t"github.com/sagernet/sing-box/adapter"\n',
                       '\t"github.com/sagernet/sing-box/adapter"\n'
                       '\tquic "github.com/sagernet/quic-go"\n',
                       'route trace import')
    data = replace_one(data, '\towner.Close()\n\treturn true\n',
                       '\tif original, ok := o.original.(net.Conn); ok {\n'
                       '\t\tquic.PolarisTraceObservedClose(original, "socket_owner", owner.Close)\n'
                       '\t} else { owner.Close() }\n\treturn true\n',
                       'socket owner close')
    data = replace_one(data,
                       '\tif c.socketOwner.detach() {\n\t\tc.Conn.Close()\n\t}\n',
                       '\tif c.socketOwner.detach() {\n'
                       '\t\tquic.PolarisTraceObservedClose(c.Conn, "tracked_detach", c.Conn.Close)\n'
                       '\t}\n',
                       'tracked detach close')
    data = replace_one(data,
                       '\tif c.socketOwner.close() {\n\t\treturn nil\n\t}\n\treturn c.Conn.Close()\n',
                       '\tif c.socketOwner.close() {\n\t\treturn nil\n\t}\n'
                       '\treturn quic.PolarisTraceObservedClose(c.Conn, "tracked_close", c.Conn.Close)\n',
                       'tracked direct close')
    return data


def patch_cmd_run(source):
    data = source.read_text()
    data = replace_one(data, '\t"github.com/sagernet/sing-box"\n',
                       '\t"github.com/sagernet/sing-box"\n'
                       '\tquic "github.com/sagernet/quic-go"\n',
                       'run trace import')
    data = replace_one(data, 'func run() error {\n\toptionsList, err := readConfig()\n',
                       'func run() error {\n'
                       '\tpolarisGraceful := false\n'
                       '\tdefer func() { quic.PolarisTraceFinalize(polarisGraceful) }()\n'
                       '\toptionsList, err := readConfig()\n',
                       'graceful run finalize')
    data = replace_one(data,
                       '\t\t\tif osSignal != syscall.SIGHUP {\n'
                       '\t\t\t\tif err != nil {\n'
                       '\t\t\t\t\tlog.Error(E.Cause(err, "sing-box did not closed properly"))\n'
                       '\t\t\t\t}\n'
                       '\t\t\t\treturn nil\n',
                       '\t\t\tif osSignal != syscall.SIGHUP {\n'
                       '\t\t\t\tif err != nil {\n'
                       '\t\t\t\t\tlog.Error(E.Cause(err, "sing-box did not closed properly"))\n'
                       '\t\t\t\t} else if osSignal == syscall.SIGTERM {\n'
                       '\t\t\t\t\tpolarisGraceful = true\n'
                       '\t\t\t\t}\n'
                       '\t\t\t\treturn nil\n',
                       'SIGTERM run exit')
    return data


def prepare(source, out, build=True, quic_reference=QUIC,
            sing_quic_reference=SING_QUIC):
    if out.parent != Path('/var/tmp') or not re.fullmatch(r'polaris-mac-masque-fdtrace-[0-9a-f]{8}', out.name):
        raise ValueError('fresh /var/tmp/polaris-mac-masque-fdtrace-<8hex> root required')
    if out.exists() or out.is_symlink():
        raise FileExistsError(out)
    check_source(source, quic_reference, sing_quic_reference)
    ldflags = link_flags(source)
    official_tags(source)
    compile_root = compiled_source_root(source, build)
    if build and (platform.system() != 'Darwin' or
                  platform.machine() not in ('arm64', 'aarch64')):
        raise RuntimeError('full CGO1 candidate requires native Darwin arm64')
    old_umask = os.umask(0o077)
    try:
        out.mkdir(mode=0o700)
        module = out / 'quic-go'
        shutil.copytree(quic_reference, module, symlinks=False)
        for root, _, _ in os.walk(module):
            Path(root).chmod(0o700)
        trace_source = (HERE / 'polaris_trace_darwin.go').read_text()
        trace_probe_guard_gate(trace_source)
        (module / 'polaris_trace_darwin.go').write_text(trace_source)
        for name in SOCKET_STATE_SOURCES:
            (module / name).write_bytes((HERE / name).read_bytes())
        for name, transform in (('sys_conn_oob.go', patch_oob),
                                ('sys_conn_msgx_darwin.go', patch_msgx)):
            target = module / name
            target.chmod(0o600)
            target.write_text(transform(target))
        modfile = out / 'build.mod'
        modfile.write_text((source / 'go.mod').read_text() +
                           f'\nreplace github.com/sagernet/quic-go => {module}\n')
        shutil.copyfile(source / 'go.sum', out / 'build.sum')
        overlays = {}
        common_overlays = {}
        for name, transform in (('route/conn.go', patch_route),
                                ('cmd/sing-box/cmd_run.go', patch_cmd_run)):
            target = out / ('overlay_' + name.replace('/', '_'))
            target.write_text(transform(source / name))
            target.chmod(0o600)
            common_overlays[str(compile_root / name)] = str(target)
        for mode in ('raw', 'dialudp'):
            overlay = out / f'client_h3_{mode}.go'
            overlay.write_text(patch_h3(source / 'transport/http/client_h3.go', mode))
            overlay.chmod(0o600)
            overlay_map = out / f'overlay_{mode}.json'
            overlay_map.write_text(json.dumps({'Replace': common_overlays | {
                str(compile_root / 'transport/http/client_h3.go'): str(overlay),
            }}, indent=2) + '\n')
            overlays[mode] = {'sourceSha256': sha(overlay.read_bytes()),
                              'overlay': str(overlay_map)}
        raw = (out / 'client_h3_raw.go').read_text()
        direct = (out / 'client_h3_dialudp.go').read_text()
        if raw.replace('c.dialer.DialContext(ctx, N.NetworkUDP, c.server)',
                       'net.DialUDP("udp4", nil, c.server.UDPAddr())').replace(
                           'PolarisTraceDial(rawConn, "raw")',
                           'PolarisTraceDial(rawConn, "dialudp")') != direct:
            raise RuntimeError('candidate modes differ outside the two declared lines')
        if 'PolarisTraceConn' in raw + direct or 'rawConn = tracedConn' in raw + direct:
            raise RuntimeError('candidate replaces the original net.Conn')
        receipt = {'sourceHead': HEAD, 'source': str(source),
                   'compiledSourceRoot': str(compile_root), 'sourcePins': PINS,
                   'linkerFlagsSha256': PINS['release/LDFLAGS'],
                   'linkerFlags': ldflags,
                   'quicPins': QUIC_PINS, 'singQuicPin': SING_QUIC_PIN,
                   'traceSourceSha256': sha((HERE / 'polaris_trace_darwin.go').read_bytes()),
                   'socketStateSourceSha256': {name: sha((HERE / name).read_bytes())
                                               for name in SOCKET_STATE_SOURCES},
                   'socketStateABI': socket_state_abi() if build else None,
                   'finalizeContract': 'process-graceful-exit-v1',
                   'finalizeSourceSha256': PINS['cmd/sing-box/cmd_run.go'],
                   'finalizeOverlaySha256': sha((out / 'overlay_cmd_sing-box_cmd_run.go').read_bytes()),
                   'commonOverlaySha256': {name: sha(Path(path).read_bytes())
                                           for name, path in common_overlays.items()},
                   'patchedQuicSha256': {name: sha((module / name).read_bytes()) for name in
                                         ('sys_conn_oob.go', 'sys_conn_msgx_darwin.go')},
                   'modes': overlays, 'buildTags': TAGS,
                   'buildProfile': BUILD_PROFILE,
                   'quicReferenceTreeSha256': QUIC_TREE_SHA256,
                   'singQuicReferenceTreeSha256': SING_QUIC_TREE_SHA256}
        if build:
            env = os.environ.copy()
            env.update(GOOS='darwin', GOARCH='arm64', CGO_ENABLED='1',
                       GOWORK='off', GOTOOLCHAIN='go1.26.8',
                       GOFLAGS='-mod=readonly')
            for mode in ('raw', 'dialudp'):
                binary = out / f'sing-box-c4-{mode}-darwin-arm64'
                subprocess.run(['go', 'build', '-trimpath', '-modfile='+str(modfile),
                                '-overlay='+overlays[mode]['overlay'], '-tags='+TAGS,
                                '-ldflags='+ldflags,
                                '-o', str(binary), './cmd/sing-box'], cwd=source,
                               env=env, check=True, timeout=900)
                binary.chmod(0o700)
                overlays[mode]['binarySha256'] = sha(binary.read_bytes())
                overlays[mode]['compileWitness'] = compile_witness(binary, mode)
                buildinfo = subprocess.run(['go', 'version', '-m', str(binary)],
                                           check=True, capture_output=True, text=True).stdout
                if 'github.com/sagernet/quic-go' not in buildinfo or 'GOOS=darwin' not in buildinfo:
                    raise RuntimeError('candidate buildinfo lost Darwin/quic-go identity')
                overlays[mode]['buildinfoSha256'] = sha(buildinfo.encode())
            if overlays['raw']['binarySha256'] == overlays['dialudp']['binarySha256']:
                raise RuntimeError('RawDialer and DialUDP binaries are byte-identical')
        (out / 'prepare.json').write_text(json.dumps(receipt, indent=2) + '\n')
        (out / 'prepare.json').chmod(0o600)
        print(json.dumps({'out':str(out), 'built':build, 'modes':receipt['modes']}))
    finally:
        os.umask(old_umask)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--no-build', action='store_true')
    parser.add_argument('--quic-reference', type=Path, default=QUIC)
    parser.add_argument('--sing-quic-reference', type=Path, default=SING_QUIC)
    args = parser.parse_args()
    prepare(args.source, args.out, not args.no_build,
            args.quic_reference, args.sing_quic_reference)
