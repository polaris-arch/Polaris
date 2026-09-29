//go:build darwin

package quic

// Private C4 observer. The original net.Conn and syscall.RawConn are unchanged.
import (
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"sync"
	"sync/atomic"
	"syscall"
	"time"
)

type PolarisTraceIdentity struct {
	Gen                uint64
	FD                 int
	firstClose         atomic.Bool
	firstTerminalErrno atomic.Int64
	dialFailureProbe   atomic.Bool
}
type polarisTraceEvent struct {
	tNS           int64
	seq           uint64
	pid           int
	nonce         string
	identity      *PolarisTraceIdentity
	event         string
	n             int
	errno         int
	detail        string
	callID        uint64
	closeBeforeNS int64
	closeAfterNS  int64
}

var polarisOrigin = time.Now()
var polarisGeneration atomic.Uint64
var polarisSequence atomic.Uint64
var polarisCloseCall atomic.Uint64
var polarisEmitted atomic.Uint64
var polarisEnqueued atomic.Uint64
var polarisWritten atomic.Uint64
var polarisDropped atomic.Uint64
var polarisWriteErrors atomic.Uint64
var polarisIdentityGaps atomic.Uint64
var polarisLateEvents atomic.Uint64
var polarisActive atomic.Int64
var polarisSealed atomic.Bool
var polarisTraceMu sync.Mutex
var polarisTraceFile *os.File
var polarisTraceRoot string
var polarisNonce string
var polarisQueue chan polarisTraceEvent
var polarisWriterDone chan struct{}
var polarisByFD = make(map[int]*PolarisTraceIdentity)        // one-time bridge
var polarisByConn = make(map[net.Conn]*PolarisTraceIdentity) // close sites only
var polarisRootPattern = regexp.MustCompile("^/Users/sway/pc-mesh-20260928-c88d395-([0-9a-f]{8})$")

func polarisOpenTrace() error {
	polarisTraceMu.Lock()
	defer polarisTraceMu.Unlock()
	if polarisTraceFile != nil {
		return nil
	}
	root := os.Getenv("HOME")
	match := polarisRootPattern.FindStringSubmatch(root)
	if match == nil {
		return errors.New("C4 trace requires nonce-scoped private HOME")
	}
	st, err := os.Lstat(root)
	if err != nil {
		return err
	}
	if !st.IsDir() || st.Mode().Perm() != 0700 || st.Mode()&os.ModeSymlink != 0 {
		return errors.New("C4 trace HOME identity/mode invalid")
	}
	if sys, ok := st.Sys().(*syscall.Stat_t); !ok || int(sys.Uid) != os.Getuid() {
		return errors.New("C4 trace HOME owner invalid")
	}
	file, err := os.OpenFile(filepath.Join(root, "masque-fdtrace.ndjson"), os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	polarisTraceRoot, polarisNonce, polarisTraceFile = root, match[1], file
	polarisQueue = make(chan polarisTraceEvent, 8192)
	polarisWriterDone = make(chan struct{})
	go polarisWriteLoop(file, polarisQueue, polarisWriterDone)
	return nil
}

func polarisWriteLoop(file *os.File, queue <-chan polarisTraceEvent, done chan<- struct{}) {
	defer close(done)
	next := uint64(1)
	pending := make(map[uint64]polarisTraceEvent)
	for event := range queue {
		if event.seq < next || pending[event.seq].identity != nil {
			polarisWriteErrors.Add(1)
			continue
		}
		if len(pending) >= cap(polarisQueue) {
			polarisWriteErrors.Add(1)
			continue
		}
		pending[event.seq] = event
		for {
			ready, ok := pending[next]
			if !ok {
				break
			}
			delete(pending, next)
			polarisWriteOne(file, ready)
			next++
		}
	}
	if len(pending) != 0 {
		polarisWriteErrors.Add(1)
	}
}
func polarisWriteOne(file *os.File, event polarisTraceEvent) {
	id := event.identity
	line, err := json.Marshal(map[string]any{
		"t_ns": event.tNS, "seq": event.seq, "pid": event.pid, "nonce": event.nonce,
		"gen": id.Gen, "fd": id.FD, "event": event.event, "n": event.n,
		"errno": event.errno, "detail": event.detail, "call_id": event.callID,
		"close_before_ns": event.closeBeforeNS, "close_after_ns": event.closeAfterNS,
	})
	if err != nil {
		polarisWriteErrors.Add(1)
		return
	}
	line = append(line, '\n')
	n, err := file.Write(line)
	if err != nil || n != len(line) {
		polarisWriteErrors.Add(1)
		return
	}
	polarisWritten.Add(1)
}
func polarisErrno(err error) int {
	if err == nil {
		return 0
	}
	var errno syscall.Errno
	if errors.As(err, &errno) {
		return int(errno)
	}
	return -1
}

// Called in RawConn callbacks: no mutex, JSON, disk I/O or blocking channel send.
func polarisEvent(id *PolarisTraceIdentity, event string, n int, err error, detail string) {
	polarisEventCall(id, event, n, err, detail, 0)
}
func polarisEventCall(id *PolarisTraceIdentity, event string, n int, err error, detail string, callID uint64) {
	polarisEventCallBounds(id, event, n, err, detail, callID, 0, 0)
}
func polarisEventCallBounds(id *PolarisTraceIdentity, event string, n int, err error, detail string, callID uint64, closeBeforeNS, closeAfterNS int64) {
	if id == nil {
		polarisIdentityGaps.Add(1)
		return
	}
	if polarisSealed.Load() {
		polarisLateEvents.Add(1)
		return
	}
	polarisActive.Add(1)
	defer polarisActive.Add(-1)
	if polarisSealed.Load() {
		polarisLateEvents.Add(1)
		return
	}
	errno := polarisErrno(err)
	row := polarisTraceEvent{
		tNS: time.Since(polarisOrigin).Nanoseconds(), seq: polarisSequence.Add(1),
		pid: os.Getpid(), nonce: polarisNonce, identity: id, event: event, n: n,
		errno: errno, detail: detail, callID: callID,
		closeBeforeNS: closeBeforeNS, closeAfterNS: closeAfterNS,
	}
	polarisEmitted.Add(1)
	select {
	case polarisQueue <- row:
		polarisEnqueued.Add(1)
		// The terminal syscall event has entered the ordered trace queue before
		// the failure-time probe is allowed to read SO_ERROR.
		if (event == "send_exit" || event == "recv_exit") && errno > 0 &&
			errno != int(syscall.EINTR) && errno != int(syscall.EAGAIN) {
			id.firstTerminalErrno.CompareAndSwap(0, int64(errno))
		}
	default:
		polarisDropped.Add(1)
	}
}

func polarisSockaddrFamily(addr syscall.Sockaddr) string {
	switch addr.(type) {
	case *syscall.SockaddrInet4:
		return "inet4"
	case *syscall.SockaddrInet6:
		return "inet6"
	default:
		return "unknown"
	}
}

func polarisPeerMatches(addr syscall.Sockaddr, planned net.Addr) string {
	udp, ok := planned.(*net.UDPAddr)
	if !ok {
		return "unknown"
	}
	switch peer := addr.(type) {
	case *syscall.SockaddrInet4:
		if udp.IP.To4() != nil && udp.Port == peer.Port &&
			net.IP(peer.Addr[:]).Equal(udp.IP) {
			return "yes"
		}
		return "no"
	case *syscall.SockaddrInet6:
		// A scoped IPv6 address needs a separate zone comparison. The fixed C4
		// endpoint is IPv4; report unknown rather than a false match for zones.
		if udp.Zone != "" || peer.ZoneId != 0 {
			return "unknown"
		}
		if udp.Port == peer.Port && net.IP(peer.Addr[:]).Equal(udp.IP) {
			return "yes"
		}
		return "no"
	default:
		return "unknown"
	}
}

// PolarisTraceDialFailureProbe runs only after qtls.DialEarly returned an
// error, with its transport stopped, and immediately before the existing
// unconditional Close of the original rawConn. It never changes that error.
func PolarisTraceDialFailureProbe(conn net.Conn, id *PolarisTraceIdentity) {
	if id == nil {
		polarisIdentityGaps.Add(1)
		return
	}
	if !id.dialFailureProbe.CompareAndSwap(false, true) {
		return
	}
	terminal := id.firstTerminalErrno.Load() != 0
	planned := conn.RemoteAddr()
	localFamily, peerFamily, peerMatch := "unknown", "unknown", "unknown"
	value := 0
	var queryErr error
	stateDetail := "not_queried_no_terminal_errno"
	stateErr := error(nil)
	stateCannotSend, statePendingError, stateValid := false, 0, false
	socket, ok := polarisFindSyscallConn(conn)
	if !ok {
		queryErr = syscall.EBADF
	} else {
		var raw syscall.RawConn
		raw, queryErr = socket.SyscallConn()
		if queryErr == nil && raw != nil {
			controlErr := raw.Control(func(fd uintptr) {
				polarisTraceMu.Lock()
				active := polarisByFD[id.FD] == id
				polarisTraceMu.Unlock()
				if int(fd) != id.FD || !active || id.firstClose.Load() {
					queryErr = syscall.EBADF
					stateDetail, stateErr = "identity_guard_errno", syscall.EBADF
					return
				}
				local, err := syscall.Getsockname(int(fd))
				if err == nil {
					localFamily = polarisSockaddrFamily(local)
				}
				peer, err := syscall.Getpeername(int(fd))
				if err == nil {
					peerFamily = polarisSockaddrFamily(peer)
					peerMatch = polarisPeerMatches(peer, planned)
				}
				if terminal {
					polarisTraceMu.Lock()
					active = polarisByFD[id.FD] == id
					polarisTraceMu.Unlock()
					if int(fd) != id.FD || !active || id.firstClose.Load() {
						localFamily, peerFamily, peerMatch = "unknown", "unknown", "unknown"
						queryErr = syscall.EBADF
						stateDetail, stateErr = "identity_guard_errno", syscall.EBADF
						return
					}
					cannotSend, pendingError, shortReturn, procErr := polarisReadSelfSocketState(int(fd))
					if shortReturn {
						stateDetail = "proc_short"
					} else if procErr != nil || pendingError < 0 || pendingError > 65535 {
						stateDetail, stateErr = "proc_errno", procErr
						if stateErr == nil {
							stateErr = syscall.EIO
						}
					} else {
						stateCannotSend, statePendingError, stateValid = cannotSend, pendingError, true
					}
					value, queryErr = syscall.GetsockoptInt(int(fd), syscall.SOL_SOCKET, syscall.SO_ERROR)
					polarisTraceMu.Lock()
					active = polarisByFD[id.FD] == id
					polarisTraceMu.Unlock()
					if int(fd) != id.FD || !active || id.firstClose.Load() {
						localFamily, peerFamily, peerMatch = "unknown", "unknown", "unknown"
						value, queryErr = 0, syscall.EBADF
						stateDetail, stateErr = "identity_guard_errno", syscall.EBADF
						stateValid = false
					}
				}
			})
			if controlErr != nil {
				queryErr = controlErr
				stateDetail, stateErr = "proc_errno", controlErr
			}
		} else if queryErr == nil {
			queryErr = syscall.EBADF
		}
	}
	polarisEvent(id, "socket_identity", 0, nil,
		"local="+localFamily+";peer="+peerFamily+";match="+peerMatch)
	if !terminal {
		polarisEvent(id, "socket_state", 0, nil, "not_queried_no_terminal_errno")
		polarisEvent(id, "socket_probe", 0, nil, "not_queried_no_terminal_errno")
		return
	}
	if stateValid && stateErr == nil {
		cannotSend := 0
		if stateCannotSend {
			cannotSend = 1
		}
		stateDetail = fmt.Sprintf("cannot_send=%d;soi_error=%d", cannotSend, statePendingError)
	}
	if stateDetail == "not_queried_no_terminal_errno" {
		stateDetail, stateErr = "proc_errno", queryErr
	}
	if stateErr != nil {
		if polarisErrno(stateErr) <= 0 || polarisErrno(stateErr) > 65535 {
			stateErr = syscall.EIO
		}
	}
	polarisEvent(id, "socket_state", 0, stateErr, stateDetail)
	if queryErr != nil {
		if polarisErrno(queryErr) <= 0 || polarisErrno(queryErr) > 65535 {
			queryErr = syscall.EIO
		}
		polarisEvent(id, "socket_probe", 0, queryErr, "getsockopt_errno")
	} else if value == 0 {
		polarisEvent(id, "socket_probe", 0, nil, "zero")
	} else if value > 0 {
		polarisEvent(id, "socket_probe", value, nil, "value")
	} else {
		polarisEvent(id, "socket_probe", 0, syscall.EIO, "getsockopt_errno")
	}
}
func polarisFD(raw syscall.RawConn) (int, error) {
	fd := -1
	err := raw.Control(func(value uintptr) { fd = int(value) })
	return fd, err
}
func polarisBindConn(conn net.Conn, id *PolarisTraceIdentity) {
	for i := 0; i < 8 && conn != nil; i++ {
		if reflect.TypeOf(conn).Comparable() {
			polarisByConn[conn] = id
		}
		upstream, ok := conn.(interface{ Upstream() any })
		if !ok {
			break
		}
		next, ok := upstream.Upstream().(net.Conn)
		if !ok {
			break
		}
		conn = next
	}
}

func polarisFindSyscallConn(conn net.Conn) (syscall.Conn, bool) {
	for i := 0; i < 8 && conn != nil; i++ {
		if socket, ok := conn.(syscall.Conn); ok {
			return socket, true
		}
		upstream, ok := conn.(interface{ Upstream() any })
		if !ok {
			break
		}
		conn, ok = upstream.Upstream().(net.Conn)
		if !ok {
			break
		}
	}
	return nil, false
}

// The same original conn is passed onward to qtls.DialEarly.
func PolarisTraceDial(conn net.Conn, mode string) (*PolarisTraceIdentity, error) {
	if err := polarisOpenTrace(); err != nil {
		return nil, err
	}
	socket, ok := polarisFindSyscallConn(conn)
	if !ok {
		return nil, errors.New("C4 connected UDP lacks syscall.Conn")
	}
	raw, err := socket.SyscallConn()
	if err != nil {
		return nil, err
	}
	fd, err := polarisFD(raw)
	if err != nil {
		return nil, err
	}
	id := &PolarisTraceIdentity{Gen: polarisGeneration.Add(1), FD: fd}
	polarisTraceMu.Lock()
	polarisByFD[fd] = id
	polarisBindConn(conn, id)
	polarisTraceMu.Unlock()
	polarisEvent(id, "connect_complete", 0, nil,
		mode+" local="+conn.LocalAddr().String()+" peer="+conn.RemoteAddr().String())
	return id, nil
}

// Called once at connected QUIC construction, never on send/recv.
func polarisBridgeRaw(raw syscall.RawConn) *PolarisTraceIdentity {
	fd, err := polarisFD(raw)
	if err != nil {
		polarisIdentityGaps.Add(1)
		return nil
	}
	polarisTraceMu.Lock()
	id := polarisByFD[fd]
	polarisTraceMu.Unlock()
	return id
}
func polarisCloseEnter(id *PolarisTraceIdentity, site string) uint64 {
	callID := polarisCloseCall.Add(1)
	if id == nil {
		polarisIdentityGaps.Add(1)
		return callID
	}
	first := 0
	// The successful CAS is inside this monotonic interval. Capturing only
	// after CAS lets a preempted winner appear later than a redundant Close,
	// which could otherwise admit an I/O enter made after closing began.
	before := time.Since(polarisOrigin).Nanoseconds()
	if id.firstClose.CompareAndSwap(false, true) {
		first = 1
	}
	after := time.Since(polarisOrigin).Nanoseconds()
	if first == 0 {
		before, after = 0, 0
	}
	polarisEventCallBounds(id, "close_enter", first, nil, site, callID, before, after)
	return callID
}
func polarisCloseExit(id *PolarisTraceIdentity, err error, site string, callID uint64) {
	polarisEventCall(id, "close_exit", 0, err, site, callID)
}
func polarisRetire(id *PolarisTraceIdentity) {
	if id == nil {
		return
	}
	polarisTraceMu.Lock()
	if polarisByFD[id.FD] == id {
		delete(polarisByFD, id.FD)
	}
	// Keep conn-object association for later redundant Close calls. A reused
	// numeric FD has a different object and generation; only its FD bridge is
	// retired here.
	polarisTraceMu.Unlock()
}
func PolarisTraceClose(conn net.Conn, id *PolarisTraceIdentity, site string) error {
	callID := polarisCloseEnter(id, site)
	err := conn.Close()
	polarisCloseExit(id, err, site, callID)
	polarisRetire(id)
	return err
}

// Observe an existing close call without changing its target or count.
func PolarisTraceObservedClose(conn net.Conn, site string, call func() error) error {
	if conn == nil || !reflect.TypeOf(conn).Comparable() {
		return call()
	}
	polarisTraceMu.Lock()
	id := polarisByConn[conn]
	polarisTraceMu.Unlock()
	if id == nil {
		return call()
	}
	callID := polarisCloseEnter(id, site)
	err := call()
	polarisCloseExit(id, err, site, callID)
	polarisRetire(id)
	return err
}
func polarisTransportClose(conn net.Conn, id *PolarisTraceIdentity) error {
	callID := polarisCloseEnter(id, "quic_transport")
	err := conn.Close()
	polarisCloseExit(id, err, "quic_transport", callID)
	polarisRetire(id)
	return err
}

// Graceful cmd_run.go exit only. The whole defer has a 1.5-second ceiling,
// below frozen stop_exact's three-second SIGKILL limit.
func PolarisTraceFinalize(graceful bool) {
	done := make(chan struct{})
	go func() {
		polarisFinalizeInner(graceful)
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(1500 * time.Millisecond):
	}
}

func polarisFinalizeInner(graceful bool) {
	polarisTraceMu.Lock()
	file, queue, done := polarisTraceFile, polarisQueue, polarisWriterDone
	root, nonce := polarisTraceRoot, polarisNonce
	polarisTraceMu.Unlock()
	if file == nil {
		return
	}
	polarisSealed.Store(true)
	deadline := time.NewTimer(time.Second)
	defer deadline.Stop()
	for polarisActive.Load() != 0 {
		select {
		case <-deadline.C:
			return
		default:
			time.Sleep(time.Millisecond)
		}
	}
	close(queue)
	select {
	case <-done:
	case <-deadline.C:
		return
	}
	if err := file.Sync(); err != nil {
		polarisWriteErrors.Add(1)
		return
	}
	if err := file.Close(); err != nil {
		polarisWriteErrors.Add(1)
		return
	}
	stream, err := os.Open(filepath.Join(root, "masque-fdtrace.ndjson"))
	if err != nil {
		return
	}
	hash := sha256.New()
	_, err = io.Copy(hash, stream)
	stream.Close()
	if err != nil || !graceful {
		return
	}
	if polarisDropped.Load() != 0 || polarisWriteErrors.Load() != 0 ||
		polarisIdentityGaps.Load() != 0 || polarisLateEvents.Load() != 0 {
		return
	}
	receipt := map[string]any{
		"nonce": nonce, "pid": os.Getpid(), "traceSha256": fmt.Sprintf("%x", hash.Sum(nil)),
		"finalized": true, "exit": "graceful", "emitted": polarisEmitted.Load(),
		"enqueued": polarisEnqueued.Load(), "written": polarisWritten.Load(),
		"dropped": polarisDropped.Load(), "writeErrors": polarisWriteErrors.Load(),
		"identityGaps": polarisIdentityGaps.Load(), "lateEvents": polarisLateEvents.Load(),
		"closeCoverage": "h3-transport-trackeddetach-socketowner-v1",
	}
	encoded, err := json.Marshal(receipt)
	if err != nil {
		return
	}
	tmp := filepath.Join(root, "masque-fdtrace.final.tmp")
	final, err := os.OpenFile(tmp, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return
	}
	n, err := final.Write(append(encoded, '\n'))
	if err != nil || n != len(encoded)+1 {
		final.Close()
		return
	}
	if err = final.Sync(); err != nil {
		final.Close()
		return
	}
	if err = final.Close(); err != nil {
		return
	}
	_ = os.Rename(tmp, filepath.Join(root, "masque-fdtrace.final.json"))
}
