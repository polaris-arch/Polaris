//go:build with_tailscale && windows

package tailscale

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"
	"unsafe"

	"github.com/sagernet/sing-box/adapter"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/experimental/directorywindows"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/service"

	"golang.org/x/sys/windows"
)

const windowsProcessTimeout = 70 * time.Second

// The fixture uses the same real strict decoder, source/VCS/envelope validation,
// whole-config activation and path resolver as box.New before native acquisition.
// This covers activation + directory IO, not tsnet/network/helper end-to-end.
type windowsTestEndpointRegistry struct{}

func (windowsTestEndpointRegistry) OptionTypes() []string { return []string{C.TypeTailscale} }

func (windowsTestEndpointRegistry) CreateOptions(kind string) (any, bool) {
	if kind == C.TypeTailscale {
		return new(option.TailscaleEndpointOptions), true
	}
	return nil, false
}

func windowsTestManagedWriter(root, server string) (*directoryLease, error) {
	nonce, err := newDirectoryNonce()
	if err != nil {
		return nil, err
	}
	logical := filepath.Join(root, "tailscale", server)
	taildrop := filepath.Join(logical, "Taildrop")
	raw, err := json.Marshal(map[string]any{"polaris_windows_directory": map[string]any{"version": 1, "root": root, "run_nonce": nonce, "expected_source": C.SourceRevision}, "endpoints": []any{map[string]any{"type": C.TypeTailscale, "tag": server, "state_directory": logical, "taildrop_directory": taildrop}}})
	if err != nil {
		return nil, err
	}
	ctx := service.ContextWith[option.EndpointOptionsRegistry](context.Background(), windowsTestEndpointRegistry{})
	var options option.Options
	if err := options.UnmarshalJSONContext(ctx, raw); err != nil {
		return nil, fmt.Errorf("test activation decode: %w", err)
	}
	prepared, err := directorywindows.Prepare(ctx, options, PoisonDirectoryDomain)
	if err != nil {
		return nil, fmt.Errorf("test activation: %w", err)
	}
	mapping := adapter.TailscaleDirectoryMappingFromContext(prepared)
	if mapping == nil || !mapping.Domain().Active() {
		return nil, fmt.Errorf("test activation produced no capability")
	}
	physical, _, ok := mapping.Resolve(logical, taildrop)
	if !ok || physical != filepath.Join(root, "tailscale-windows-v1", server) {
		return nil, fmt.Errorf("test activation mapping mismatch")
	}
	actual, lease, err := acquireMappedDirectory(logical, physical)
	if err != nil {
		return nil, err
	}
	if actual != physical || !lease.Managed {
		_ = lease.releaseHandles()
		return nil, fmt.Errorf("test activation did not select managed target")
	}
	return lease, nil
}

func windowsTestSignal(path, nonce string) error {
	temporary := path + ".tmp"
	if err := os.WriteFile(temporary, []byte(nonce), 0o600); err != nil {
		return err
	}
	return os.Rename(temporary, path)
}

// Read only test IPC files, which are published by temp-close-rename. A renamed
// name can become visible before MoveFileEx closes its DELETE handle. Sharing
// DELETE here permits that overlap; production directory custody must not do so.
func windowsTestReadSignal(path string) ([]byte, error) {
	wide, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return nil, &os.PathError{Op: "open", Path: path, Err: err}
	}
	h, err := windows.CreateFile(wide, windows.GENERIC_READ, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE, nil, windows.OPEN_EXISTING, windows.FILE_ATTRIBUTE_NORMAL, 0)
	if err != nil {
		return nil, &os.PathError{Op: "open", Path: path, Err: err}
	}
	f := os.NewFile(uintptr(h), path)
	if f == nil {
		return nil, errors.Join(fmt.Errorf("wrap test signal handle: %s", path), windows.CloseHandle(h))
	}
	data, readErr := io.ReadAll(f)
	return data, errors.Join(readErr, f.Close())
}

func windowsTestHasSignal(path, nonce string) (bool, error) {
	data, err := windowsTestReadSignal(path)
	if os.IsNotExist(err) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return string(data) == nonce, nil
}

func windowsProcessFile(root, nonce, phase string) string {
	return filepath.Join(root, "process-"+nonce+"."+phase)
}

type (
	windowsProcessReady  struct{ Nonce, Mode, Identity, Incarnation, Epoch, ControlName, LockName string }
	windowsProcessOutput struct {
		mu     sync.Mutex
		buffer bytes.Buffer
	}
)

func (b *windowsProcessOutput) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buffer.Write(p)
}

func (b *windowsProcessOutput) text() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buffer.String()
}

type windowsTestProcess struct {
	cmd         *exec.Cmd
	cancel      context.CancelFunc
	done        chan struct{}
	err         error
	output      *windowsProcessOutput
	root, nonce string
	ready       windowsProcessReady
}

func windowsTestDecodeReady(data []byte, nonce, mode string) (windowsProcessReady, error) {
	var ready windowsProcessReady
	if err := json.Unmarshal(data, &ready); err != nil {
		return ready, err
	}
	if ready.Nonce != nonce || ready.Mode != mode {
		return ready, fmt.Errorf("child ready did not bind request")
	}
	if mode != "reject" && (ready.Identity == "" || !validDirectoryNonce(ready.Incarnation) || !validDirectoryNonce(ready.Epoch)) {
		return ready, fmt.Errorf("child ready lacked physical generation")
	}
	return ready, nil
}

func windowsTestStartProcess(t *testing.T, root, mode string, extra ...string) *windowsTestProcess {
	t.Helper()
	nonce, err := newDirectoryNonce()
	if err != nil {
		t.Fatal(err)
	}
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), windowsProcessTimeout)
	cmd := exec.CommandContext(ctx, exe, "-test.run=^TestWindowsDirectoryProcessChild$", "-test.v", "-test.timeout=65s")
	for _, e := range os.Environ() {
		if !strings.HasPrefix(strings.ToUpper(e), "POLARIS_DIRECTORY_CHILD_") {
			cmd.Env = append(cmd.Env, e)
		}
	}
	cmd.Env = append(cmd.Env, "POLARIS_DIRECTORY_CHILD_ROOT="+root, "POLARIS_DIRECTORY_CHILD_NONCE="+nonce, "POLARIS_DIRECTORY_CHILD_MODE="+mode)
	cmd.Env = append(cmd.Env, extra...)
	output := new(windowsProcessOutput)
	cmd.Stdout, cmd.Stderr = output, output
	p := &windowsTestProcess{cmd: cmd, cancel: cancel, done: make(chan struct{}), output: output, root: root, nonce: nonce}
	if err := cmd.Start(); err != nil {
		cancel()
		t.Fatal(err)
	}
	go func() { p.err = cmd.Wait(); close(p.done) }()
	t.Cleanup(func() {
		cancel()
		select {
		case <-p.done:
		case <-time.After(10 * time.Second):
			t.Error("test child did not terminate after cancellation")
		}
	})
	deadline := time.NewTimer(15 * time.Second)
	defer deadline.Stop()
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	for {
		data, err := windowsTestReadSignal(windowsProcessFile(root, nonce, "ready"))
		if err == nil {
			p.ready, err = windowsTestDecodeReady(data, nonce, mode)
			if err != nil {
				t.Fatal(err)
			}
			return p
		}
		if !os.IsNotExist(err) {
			t.Fatal(err)
		}
		select {
		case <-p.done:
			if _, err := os.Stat(windowsProcessFile(root, nonce, "ready")); err == nil {
				continue
			}
			t.Fatalf("child exited without native ready: %v\n%s", p.err, output.text())
		case <-deadline.C:
			t.Fatalf("child ready timeout\n%s", output.text())
		case <-ticker.C:
		}
	}
}

// Hold the DELETE access used during rename without relying on scheduler
// timing. The ordinary reader must fail while the IPC reader reads all bytes.
func TestWindowsDirectorySignalReadDuringRename(t *testing.T) {
	path := filepath.Join(t.TempDir(), "ready")
	nonce := strings.Repeat("1", 32)
	ready := windowsProcessReady{Nonce: nonce, Mode: "writer", Identity: "fixture", Incarnation: strings.Repeat("2", 32), Epoch: strings.Repeat("3", 32)}
	data, err := json.Marshal(ready)
	if err != nil {
		t.Fatal(err)
	}
	if err := windowsTestSignal(path, string(data)); err != nil {
		t.Fatal(err)
	}
	wide, err := windows.UTF16PtrFromString(path)
	if err != nil {
		t.Fatal(err)
	}
	h, err := windows.CreateFile(wide, windows.DELETE, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE, nil, windows.OPEN_EXISTING, windows.FILE_ATTRIBUTE_NORMAL, 0)
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := windows.CloseHandle(h); err != nil {
			t.Error(err)
		}
	}()
	if _, err := os.ReadFile(path); !errors.Is(err, windows.ERROR_SHARING_VIOLATION) {
		t.Fatalf("ordinary reader error = %v, want sharing violation", err)
	}
	got, err := windowsTestReadSignal(path)
	if err != nil || !bytes.Equal(got, data) {
		t.Fatalf("IPC reader did not read complete published bytes: %v", err)
	}
	decoded, err := windowsTestDecodeReady(got, nonce, "writer")
	if err != nil || decoded != ready {
		t.Fatalf("IPC reader lost ready binding: %v", err)
	}
	if ok, err := windowsTestHasSignal(path, string(data)); err != nil || !ok {
		t.Fatalf("signal reader did not share rename handle: %v", err)
	}
	if ok, err := windowsTestHasSignal(path, nonce); err != nil || ok {
		t.Fatalf("signal reader accepted wrong payload: %v", err)
	}
}

func TestWindowsDirectorySignalValidation(t *testing.T) {
	root := t.TempDir()
	missing := filepath.Join(root, "missing")
	if _, err := windowsTestReadSignal(missing); !os.IsNotExist(err) {
		t.Fatalf("missing signal error = %v, want not exist", err)
	}
	if ok, err := windowsTestHasSignal(missing, "nonce"); err != nil || ok {
		t.Fatalf("missing signal was accepted: %v", err)
	}
	nonce := strings.Repeat("1", 32)
	signal := filepath.Join(root, "signal")
	if err := windowsTestSignal(signal, nonce); err != nil {
		t.Fatal(err)
	}
	if ok, err := windowsTestHasSignal(signal, nonce); err != nil || !ok {
		t.Fatalf("valid signal rejected: %v", err)
	}
	if ok, err := windowsTestHasSignal(signal, "invalid"); err != nil || ok {
		t.Fatalf("wrong nonce accepted: %v", err)
	}
	if ok, err := windowsTestHasSignal("invalid\x00path", nonce); err == nil || ok {
		t.Fatal("non-missing open error was suppressed")
	}
	rejected := []byte(`{"Nonce":"` + nonce + `","Mode":"reject"}`)
	if _, err := windowsTestDecodeReady(rejected, nonce, "reject"); err != nil {
		t.Fatalf("native rejection ready requires no generation: %v", err)
	}
	valid := windowsProcessReady{Nonce: nonce, Mode: "writer", Identity: "fixture", Incarnation: strings.Repeat("2", 32), Epoch: strings.Repeat("3", 32)}
	for _, field := range []string{"json", "nonce", "mode", "identity", "incarnation", "epoch"} {
		t.Run(field, func(t *testing.T) {
			ready := valid
			switch field {
			case "nonce":
				ready.Nonce = "invalid"
			case "mode":
				ready.Mode = "retirement"
			case "identity":
				ready.Identity = ""
			case "incarnation":
				ready.Incarnation = "invalid"
			case "epoch":
				ready.Epoch = "invalid"
			}
			data, err := json.Marshal(ready)
			if err != nil {
				t.Fatal(err)
			}
			if field == "json" {
				data = []byte("{")
			}
			path := filepath.Join(root, field)
			if err := windowsTestSignal(path, string(data)); err != nil {
				t.Fatal(err)
			}
			got, err := windowsTestReadSignal(path)
			if err != nil {
				t.Fatal(err)
			}
			if _, err := windowsTestDecodeReady(got, nonce, "writer"); err == nil {
				t.Fatal("invalid ready accepted")
			}
			if ok, err := windowsTestHasSignal(path, nonce); err != nil || ok {
				t.Fatalf("invalid signal accepted: %v", err)
			}
		})
	}
}

func (p *windowsTestProcess) signal(t *testing.T, phase string) {
	t.Helper()
	if err := windowsTestSignal(windowsProcessFile(p.root, p.nonce, phase), p.nonce); err != nil {
		t.Fatal(err)
	}
}

func (p *windowsTestProcess) awaitSignal(t *testing.T, phase string) {
	t.Helper()
	deadline := time.NewTimer(15 * time.Second)
	defer deadline.Stop()
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	path := windowsProcessFile(p.root, p.nonce, phase)
	for {
		ready, err := windowsTestHasSignal(path, p.nonce)
		if err != nil {
			t.Fatal(err)
		}
		if ready {
			return
		}
		select {
		case <-p.done:
			if ready, err := windowsTestHasSignal(path, p.nonce); err == nil && ready {
				return
			}
			t.Fatalf("child exited without %s: %v\n%s", phase, p.err, p.output.text())
		case <-deadline.C:
			t.Fatalf("child %s timeout\n%s", phase, p.output.text())
		case <-ticker.C:
		}
	}
}

func (p *windowsTestProcess) wait(t *testing.T, killed bool) {
	t.Helper()
	select {
	case <-p.done:
		if killed && p.err == nil {
			t.Fatal("crash returned ordinary success")
		}
		if !killed && p.err != nil {
			t.Fatalf("child failed: %v\n%s", p.err, p.output.text())
		}
	case <-time.After(15 * time.Second):
		t.Fatalf("child exit timeout\n%s", p.output.text())
	}
}

func (p *windowsTestProcess) close(t *testing.T) {
	t.Helper()
	p.signal(t, "close")
	p.awaitSignal(t, "closed")
	p.wait(t, false)
}

func (p *windowsTestProcess) kill(t *testing.T) {
	t.Helper()
	if err := p.cmd.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	p.wait(t, true)
}

// Real test-only retirement custody: root admission EX + lifetime EX + original
// target DELETE handle. It performs no deletion or synthetic production receipt.
func windowsTestRetirementCustody(path string) (_ *directoryLease, resultErr error) {
	l, err := openDirectoryAdmission(filepath.Dir(path))
	if err != nil {
		return nil, err
	}
	defer func() {
		if resultErr != nil {
			resultErr = errors.Join(resultErr, l.releaseHandles())
		}
	}()
	if unsafe, err := l.domainUnsafe(); err != nil || unsafe {
		return nil, fmt.Errorf("retirement revoked/unknown: %v", err)
	}
	l.targetName = filepath.Base(path)
	read, err := l.openPrivate(l.root, l.targetName, true, windows.FILE_OPEN, false)
	if err != nil {
		return nil, err
	}
	l.targetID, err = windowsInspect(read, true)
	err = errors.Join(err, windows.CloseHandle(read))
	if err != nil {
		return nil, err
	}
	marker, err := l.readMarker(directoryControlStem(l.targetName) + ".jsonl")
	if err != nil {
		return nil, err
	}
	if marker.Phase != "Open" || marker.DirectoryVolume != l.targetID.Volume || marker.DirectoryFileID != l.targetID.hex() {
		return nil, fmt.Errorf("retirement generation mismatch")
	}
	l.lockName = "directory-" + l.targetID.key() + ".lock"
	l.lifetime, err = l.openPrivate(l.control, l.lockName, false, windows.FILE_OPEN, true)
	if err != nil {
		return nil, err
	}
	l.lifetimeID, err = windowsInspect(l.lifetime, false)
	if err != nil {
		return nil, err
	}
	if err := windowsLock(l.lifetime, true); err != nil {
		return nil, err
	}
	l.lifetimeHeld = true
	wide, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return nil, err
	}
	l.target, err = windows.CreateFile(wide, windows.DELETE|windows.READ_CONTROL|windows.GENERIC_READ, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE, nil, windows.OPEN_EXISTING, windows.FILE_FLAG_BACKUP_SEMANTICS|windows.FILE_FLAG_OPEN_REPARSE_POINT, 0)
	if err != nil {
		return nil, err
	}
	id, err := windowsInspect(l.target, true)
	if err != nil {
		return nil, err
	}
	if id != l.targetID {
		return nil, fmt.Errorf("retirement target replaced")
	}
	if err := windowsRequirePrivate(l.target, l.ownerSID); err != nil {
		return nil, err
	}
	stamp, err := l.readPrivate(l.target, directoryIncarnationStamp, 34)
	if err != nil {
		return nil, err
	}
	if string(stamp) != marker.Incarnation+"\n" {
		return nil, fmt.Errorf("retirement stamp mismatch")
	}
	l.Identity = windowsDirectoryIdentity(l.rootID, l.targetID)
	l.Incarnation, l.Epoch = marker.Incarnation, marker.DirectoryEpoch
	return l, nil
}

func TestWindowsDirectoryProcessChild(t *testing.T) {
	root, nonce, mode := os.Getenv("POLARIS_DIRECTORY_CHILD_ROOT"), os.Getenv("POLARIS_DIRECTORY_CHILD_NONCE"), os.Getenv("POLARIS_DIRECTORY_CHILD_MODE")
	if root == "" {
		t.Skip("independent process entry needs explicit fixture")
	}
	if !validDirectoryNonce(nonce) {
		t.Fatal("invalid process nonce")
	}
	path := filepath.Join(root, "tailscale-windows-v1", "node")
	var lease *directoryLease
	var err error
	switch mode {
	case "writer", "reject":
		lease, err = windowsTestManagedWriter(root, "node")
	case "retirement":
		lease, err = windowsTestRetirementCustody(path)
	default:
		t.Fatal("invalid child mode")
	}
	ready := windowsProcessReady{Nonce: nonce, Mode: mode}
	if mode == "reject" {
		if err == nil {
			_ = lease.releaseHandles()
			t.Fatal("expected actual admission rejection")
		}
		if strings.Contains(err.Error(), "test activation") {
			t.Fatal("activation failure cannot substitute for native negative control", err)
		}
		if want := os.Getenv("POLARIS_DIRECTORY_CHILD_REJECT_CONTAINS"); want != "" && !strings.Contains(err.Error(), want) {
			t.Fatalf("wrong native rejection: %v", err)
		}
	} else {
		if err != nil {
			t.Fatal(err)
		}
		defer lease.releaseHandles()
		ready.Identity, ready.Incarnation, ready.Epoch = lease.Identity, lease.Incarnation, lease.Epoch
		ready.ControlName, ready.LockName = lease.controlName, lease.lockName
	}
	data, err := json.Marshal(ready)
	if err != nil {
		t.Fatal(err)
	}
	if err := windowsTestSignal(windowsProcessFile(root, nonce, "ready"), string(data)); err != nil {
		t.Fatal(err)
	}
	if mode == "reject" {
		return
	}
	deadline := time.NewTimer(60 * time.Second)
	defer deadline.Stop()
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	blocked, poisoned := false, false
	for {
		has := func(phase string) bool {
			yes, err := windowsTestHasSignal(windowsProcessFile(root, nonce, phase), nonce)
			if err != nil {
				t.Fatal(err)
			}
			return yes
		}
		if has("failed-revoke") && !blocked {
			if err := prepareDirectoryLeaseClose(lease, errors.New("injected drain failure")); err == nil {
				t.Fatal("another process's admission EX did not block revoke")
			}
			if lease.target == 0 || lease.lifetime == 0 || !lease.lifetimeHeld {
				t.Fatal("failed revoke released writer custody")
			}
			blocked = true
			if err := windowsTestSignal(windowsProcessFile(root, nonce, "revoke-blocked"), nonce); err != nil {
				t.Fatal(err)
			}
		}
		if has("poison") && !poisoned {
			if err := PoisonDirectoryDomain(filepath.Dir(path)); err != nil {
				t.Fatal(err)
			}
			poisoned = true
			if err := windowsTestSignal(windowsProcessFile(root, nonce, "poisoned"), nonce); err != nil {
				t.Fatal(err)
			}
		}
		if has("close") {
			if mode == "retirement" {
				err = lease.releaseHandles()
			} else {
				if blocked {
					if err := prepareDirectoryLeaseClose(lease, errors.New("retry failed drain revocation")); err != nil {
						t.Fatal(err)
					}
					poisoned = true
				}
				err = lease.Close()
				wantError := poisoned || os.Getenv("POLARIS_DIRECTORY_CHILD_EXPECT_CLOSE_ERROR") == "1"
				if (err != nil) != wantError {
					t.Fatalf("unexpected real Close result: %v", err)
				}
				err = nil
			}
			if err != nil {
				t.Fatal(err)
			}
			if lease.target != 0 || lease.lifetime != 0 {
				t.Fatal("close retained uncertain custody")
			}
			if err := windowsTestSignal(windowsProcessFile(root, nonce, "closed"), nonce); err != nil {
				t.Fatal(err)
			}
			return
		}
		select {
		case <-deadline.C:
			t.Fatal("child command deadline expired")
		case <-ticker.C:
		}
	}
}

func TestWindowsDirectoryCrossProcessWriterAndRetirementExclude(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, "tailscale-windows-v1", "node")
	writer := windowsTestStartProcess(t, root, "writer")
	if l, err := windowsTestRetirementCustody(path); err == nil {
		_ = l.releaseHandles()
		t.Fatal("retirement passed live independent writer")
	}
	writer.close(t)
	retired := windowsTestStartProcess(t, root, "retirement")
	if l, err := acquireDirectoryLease(path); err == nil {
		_ = l.Close()
		t.Fatal("writer passed independent retirement")
	}
	retired.close(t)
	next := windowsTestStartProcess(t, root, "writer")
	next.close(t)
}

func TestWindowsDirectoryCrossProcessRevokeFailureRetainsOldLease(t *testing.T) {
	root := t.TempDir()
	writer := windowsTestStartProcess(t, root, "writer")
	admission, err := openDirectoryAdmission(filepath.Join(root, "tailscale-windows-v1"))
	if err != nil {
		t.Fatal(err)
	}
	defer admission.releaseHandles()
	writer.signal(t, "failed-revoke")
	writer.awaitSignal(t, "revoke-blocked")
	h, err := admission.openPrivate(admission.control, writer.ready.LockName, false, windows.FILE_OPEN, true)
	if err != nil {
		t.Fatal(err)
	}
	lockErr := windowsLock(h, true)
	if lockErr == nil {
		_ = windowsUnlock(h)
	}
	closeErr := windows.CloseHandle(h)
	if closeErr != nil {
		t.Fatal(closeErr)
	}
	if lockErr == nil {
		t.Fatal("old writer lease released after failed revoke")
	}
	if err := admission.releaseHandles(); err != nil {
		t.Fatal(err)
	}
	writer.close(t)
	rejected := windowsTestStartProcess(t, root, "reject", "POLARIS_DIRECTORY_CHILD_REJECT_CONTAINS=permanently revoked")
	rejected.wait(t, false)
}

func TestWindowsDirectoryCrossProcessCrashReopensSameGeneration(t *testing.T) {
	root := t.TempDir()
	old := windowsTestStartProcess(t, root, "writer")
	before := old.ready
	old.kill(t)
	next := windowsTestStartProcess(t, root, "writer")
	if next.ready.Identity != before.Identity || next.ready.Incarnation != before.Incarnation || next.ready.Epoch == before.Epoch {
		t.Fatal("crash reopen changed generation or reused epoch")
	}
	next.close(t)
}

func TestWindowsDirectoryCrossProcessCrashCannotUndoRevocation(t *testing.T) {
	root := t.TempDir()
	old := windowsTestStartProcess(t, root, "writer")
	old.signal(t, "poison")
	old.awaitSignal(t, "poisoned")
	old.kill(t)
	rejected := windowsTestStartProcess(t, root, "reject", "POLARIS_DIRECTORY_CHILD_REJECT_CONTAINS=permanently revoked")
	rejected.wait(t, false)
}

func TestWindowsDirectoryCrossProcessReplacementCannotAdoptOldIdentity(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, "tailscale-windows-v1", "node")
	old := windowsTestStartProcess(t, root, "writer")
	identity := old.ready.Identity
	old.close(t)
	if err := os.Rename(path, path+"-original"); err != nil {
		t.Fatal(err)
	}
	admission, err := openDirectoryAdmission(filepath.Dir(path))
	if err != nil {
		t.Fatal(err)
	}
	h, err := admission.openPrivate(admission.root, "node", true, windows.FILE_CREATE, false)
	if err != nil {
		_ = admission.releaseHandles()
		t.Fatal(err)
	}
	id, err := windowsInspect(h, true)
	err = errors.Join(err, windows.CloseHandle(h), admission.releaseHandles())
	if err != nil {
		t.Fatal(err)
	}
	if strings.HasSuffix(identity, ":"+fmt.Sprint(id.Volume)+":"+id.hex()) {
		t.Fatal("replacement fixture reused physical identity")
	}
	rejected := windowsTestStartProcess(t, root, "reject")
	rejected.wait(t, false)
}

// SetSecurityInfo can record conversion to the current inheritance model by
// setting AUTO_INHERITED, even while PROTECTED remains set. Permit only that
// one-way metadata transition; compare the ordered ACE bytes without SDDL.
func windowsTestRestoredDACL(original, restored *windows.SECURITY_DESCRIPTOR) error {
	if original == nil || restored == nil || !original.IsValid() || !restored.IsValid() {
		return fmt.Errorf("invalid DACL security descriptor")
	}
	originalControl, _, err := original.Control()
	if err != nil {
		return fmt.Errorf("read original control: %w", err)
	}
	restoredControl, _, err := restored.Control()
	if err != nil {
		return fmt.Errorf("read restored control: %w", err)
	}
	const required = windows.SE_DACL_PRESENT | windows.SE_DACL_PROTECTED
	if originalControl&required != required || restoredControl&required != required {
		return fmt.Errorf("DACL must remain present and protected")
	}
	if restoredControl != originalControl && restoredControl != originalControl|windows.SE_DACL_AUTO_INHERITED {
		return fmt.Errorf("unexpected DACL control transition")
	}
	originalACL, _, err := original.DACL()
	if err != nil {
		return fmt.Errorf("read original ACE list: %w", err)
	}
	restoredACL, _, err := restored.DACL()
	if err != nil {
		return fmt.Errorf("read restored ACE list: %w", err)
	}
	if originalACL == nil || restoredACL == nil {
		return fmt.Errorf("null DACL cannot verify private permission restoration")
	}
	if originalACL.AceCount != restoredACL.AceCount {
		return fmt.Errorf("restored ACE count differs")
	}
	for i := range uint32(originalACL.AceCount) {
		var originalACE, restoredACE *windows.ACCESS_ALLOWED_ACE
		if err := windows.GetAce(originalACL, i, &originalACE); err != nil {
			return fmt.Errorf("read original ACE %d: %w", i, err)
		}
		if err := windows.GetAce(restoredACL, i, &restoredACE); err != nil {
			return fmt.Errorf("read restored ACE %d: %w", i, err)
		}
		originalBytes := unsafe.Slice((*byte)(unsafe.Pointer(originalACE)), originalACE.Header.AceSize)
		restoredBytes := unsafe.Slice((*byte)(unsafe.Pointer(restoredACE)), restoredACE.Header.AceSize)
		if !bytes.Equal(originalBytes, restoredBytes) {
			return fmt.Errorf("restored ACE %d differs", i)
		}
	}
	return nil
}

func TestWindowsDirectoryDACLRestorationComparison(t *testing.T) {
	parse := func(sddl string) *windows.SECURITY_DESCRIPTOR {
		t.Helper()
		sd, err := windows.SecurityDescriptorFromString(sddl)
		if err != nil {
			t.Fatal(err)
		}
		return sd
	}
	original := parse("D:P(A;;FA;;;SY)(A;;FR;;;BA)")
	for _, tc := range []struct {
		name string
		sddl string
		want bool
	}{
		{"exact", "D:P(A;;FA;;;SY)(A;;FR;;;BA)", true},
		{"auto-inherited-metadata", "D:PAI(A;;FA;;;SY)(A;;FR;;;BA)", true},
		{"unprotected", "D:(A;;FA;;;SY)(A;;FR;;;BA)", false},
		{"auto-inherit-request", "D:PAR(A;;FA;;;SY)(A;;FR;;;BA)", false},
		{"missing-dacl", "O:SY", false},
		{"null-dacl", "D:PNO_ACCESS_CONTROL", false},
		{"empty-dacl", "D:P", false},
		{"added-ace", "D:P(A;;FA;;;SY)(A;;FR;;;BA)(A;;FW;;;WD)", false},
		{"removed-ace", "D:P(A;;FA;;;SY)", false},
		{"changed-rights", "D:P(A;;FA;;;SY)(A;;FW;;;BA)", false},
		{"changed-sid", "D:P(A;;FA;;;SY)(A;;FR;;;WD)", false},
		{"changed-inheritance-flags", "D:P(A;OI;FA;;;SY)(A;;FR;;;BA)", false},
		{"changed-ace-type", "D:P(D;;FA;;;SY)(A;;FR;;;BA)", false},
		{"changed-order", "D:P(A;;FR;;;BA)(A;;FA;;;SY)", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if err := windowsTestRestoredDACL(original, parse(tc.sddl)); (err == nil) != tc.want {
				t.Fatalf("restoration comparison accepted=%v, want %v: %v", err == nil, tc.want, err)
			}
		})
	}
	if err := windowsTestRestoredDACL(parse("D:PAI(A;;FA;;;SY)(A;;FR;;;BA)"), original); err == nil {
		t.Fatal("loss of auto-inherited metadata accepted")
	}
}

// Copy ACE storage before mutation so later diagnostics never depend on an
// aliased security descriptor. Raw bytes stay in memory and are never logged.
func windowsTestCopyACEs(sd *windows.SECURITY_DESCRIPTOR) ([][]byte, error) {
	if sd == nil || !sd.IsValid() {
		return nil, fmt.Errorf("invalid diagnostic security descriptor")
	}
	acl, _, err := sd.DACL()
	if err != nil || acl == nil {
		return nil, fmt.Errorf("unavailable diagnostic DACL: %v", err)
	}
	aces := make([][]byte, acl.AceCount)
	var resultErr error
	for i := range uint32(acl.AceCount) {
		var ace *windows.ACCESS_ALLOWED_ACE
		if err := windows.GetAce(acl, i, &ace); err != nil {
			resultErr = errors.Join(resultErr, fmt.Errorf("read diagnostic ACE %d: %w", i, err))
			continue
		}
		if ace == nil || ace.Header.AceSize < uint16(unsafe.Sizeof(ace.Header)) || uint32(ace.Header.AceSize) > sd.Length() {
			resultErr = errors.Join(resultErr, fmt.Errorf("invalid diagnostic ACE %d size", i))
			continue
		}
		aces[i] = bytes.Clone(unsafe.Slice((*byte)(unsafe.Pointer(ace)), ace.Header.AceSize))
	}
	runtime.KeepAlive(sd)
	return aces, resultErr
}

func windowsTestACEDifferences(phase string, original, current [][]byte) []string {
	describe := func(ace []byte, present bool) string {
		if !present {
			return "missing"
		}
		if len(ace) < 8 {
			return fmt.Sprintf("truncated bytes=%d", len(ace))
		}
		return fmt.Sprintf("type=%#x flags=%#x mask=%#x size=%d bytes=%d", ace[0], ace[1], binary.LittleEndian.Uint32(ace[4:8]), binary.LittleEndian.Uint16(ace[2:4]), len(ace))
	}
	messages := []string{fmt.Sprintf("DACL %s: original ACE count=%d, current ACE count=%d", phase, len(original), len(current))}
	for i := range max(len(original), len(current)) {
		var a, b []byte
		hasOriginal, hasCurrent := i < len(original), i < len(current)
		if hasOriginal {
			a = original[i]
		}
		if hasCurrent {
			b = current[i]
		}
		// Simple allow/deny ACEs put the SID after the header and access mask.
		// Compare it without logging an SID, SDDL, digest or raw payload.
		simple := func(ace []byte) bool {
			return len(ace) >= 16 && int(binary.LittleEndian.Uint16(ace[2:4])) == len(ace) && (ace[0] == windows.ACCESS_ALLOWED_ACE_TYPE || ace[0] == windows.ACCESS_DENIED_ACE_TYPE)
		}
		sidComparable := simple(a) && simple(b)
		matchedOriginal := -1
		if hasCurrent {
			for j, candidate := range original {
				if bytes.Equal(candidate, b) {
					matchedOriginal = j
					break
				}
			}
		}
		messages = append(messages, fmt.Sprintf("DACL %s ACE %d: original %s; current %s; SID comparable=%v equal=%v; full bytes equal=%v; exact original index=%d", phase, i, describe(a, hasOriginal), describe(b, hasCurrent), sidComparable, sidComparable && bytes.Equal(a[8:], b[8:]), hasOriginal && hasCurrent && bytes.Equal(a, b), matchedOriginal))
	}
	return messages
}

func TestWindowsDirectoryDACLDiagnostics(t *testing.T) {
	privatePayload := "PRIVATE-SID-BYTES"
	ace := append([]byte{windows.ACCESS_ALLOWED_ACE_TYPE, 3, 0, 0, 0xff, 1, 0x1f, 0}, []byte(privatePayload)...)
	binary.LittleEndian.PutUint16(ace[2:4], uint16(len(ace)))
	changed := bytes.Clone(ace)
	changed[1] = 0
	changed[len(changed)-1] ^= 1
	for _, tc := range []struct {
		name     string
		original [][]byte
		current  [][]byte
		count    int
	}{
		{"all-aces", [][]byte{ace, ace, ace}, [][]byte{changed, ace, changed}, 3},
		{"reordered", [][]byte{ace, changed}, [][]byte{changed, ace}, 2},
		{"added", [][]byte{ace}, [][]byte{ace, changed}, 2},
		{"missing", [][]byte{ace, changed}, [][]byte{ace}, 2},
		{"truncated", [][]byte{nil, {0}, {0, 0, 0, 0}}, [][]byte{{}, {}, {}}, 3},
		{"invalid-size", [][]byte{{0, 0, 0, 0, 0, 0, 0, 0}}, [][]byte{ace}, 1},
	} {
		t.Run(tc.name, func(t *testing.T) {
			messages := windowsTestACEDifferences("probe", tc.original, tc.current)
			if len(messages) != tc.count+1 {
				t.Fatalf("diagnostics omitted ACEs: got %d messages", len(messages))
			}
			for i := range tc.count {
				if !strings.Contains(messages[i+1], fmt.Sprintf("ACE %d:", i)) {
					t.Fatalf("diagnostics omitted ACE index %d", i)
				}
			}
			for _, message := range messages {
				if strings.Contains(message, privatePayload) || strings.Contains(message, "S-1-") || strings.Contains(message, "D:P") {
					t.Fatal("diagnostics exposed private payload or identity")
				}
			}
		})
	}
	messages := windowsTestACEDifferences("probe", [][]byte{ace, ace}, [][]byte{changed, ace})
	if !strings.Contains(messages[1], "SID comparable=true equal=false") || !strings.Contains(messages[1], "full bytes equal=false") || !strings.Contains(messages[2], "full bytes equal=true") {
		t.Fatal("diagnostics lost SID equality or later-ACE evidence")
	}
}

func windowsTestBroadenDACL(t *testing.T, path string) {
	t.Helper()
	wide, err := windows.UTF16PtrFromString(path)
	if err != nil {
		t.Fatal(err)
	}
	const requestedAccess = windows.READ_CONTROL | windows.WRITE_DAC
	h, err := windows.CreateFile(wide, requestedAccess, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE, nil, windows.OPEN_EXISTING, windows.FILE_FLAG_OPEN_REPARSE_POINT, 0)
	if err != nil {
		t.Fatalf("open DACL mutation handle (requested access %#x): %v", requestedAccess, err)
	}
	defer windows.CloseHandle(h)
	original, err := windows.GetSecurityInfo(h, windows.SE_FILE_OBJECT, windows.DACL_SECURITY_INFORMATION)
	if err != nil {
		t.Fatalf("read original DACL (requested access %#x): %v", requestedAccess, err)
	}
	originalControl, _, err := original.Control()
	if err != nil {
		t.Fatalf("read original DACL control: %v", err)
	}
	if err := windowsTestRestoredDACL(original, original); err != nil {
		t.Fatalf("validate original DACL: %v", err)
	}
	if originalControl&windows.SE_SELF_RELATIVE == 0 {
		t.Fatal("original DACL snapshot must be self-relative")
	}
	originalStorage := bytes.Clone(unsafe.Slice((*byte)(unsafe.Pointer(original)), original.Length()))
	originalACEs, err := windowsTestCopyACEs(original)
	if err != nil {
		t.Fatal(err)
	}
	owner, err := windowsCurrentSID()
	if err != nil {
		t.Fatal(err)
	}
	changed, err := windows.SecurityDescriptorFromString("D:P(A;;FA;;;" + owner + ")(A;;FA;;;SY)(A;;FW;;;WD)")
	if err != nil {
		t.Fatal(err)
	}
	acl, _, err := changed.DACL()
	if err != nil {
		t.Fatal(err)
	}
	flags := windows.SECURITY_INFORMATION(windows.DACL_SECURITY_INFORMATION | windows.PROTECTED_DACL_SECURITY_INFORMATION)
	if err := windows.SetSecurityInfo(h, windows.SE_FILE_OBJECT, flags, nil, nil, acl, nil); err != nil {
		t.Fatalf("broaden DACL (requested access %#x, original control %#x, security info %#x): %v", requestedAccess, originalControl, flags, err)
	}
	t.Cleanup(func() {
		defer runtime.KeepAlive(original)
		defer runtime.KeepAlive(originalStorage)
		originalUnchanged := bytes.Equal(originalStorage, unsafe.Slice((*byte)(unsafe.Pointer(original)), len(originalStorage)))
		if !originalUnchanged {
			t.Error("original DACL storage changed before restoration")
		}
		acl, _, err := original.DACL()
		if err != nil {
			t.Errorf("extract original DACL for restore (original control %#x): %v", originalControl, err)
			return
		}
		// Match the mutation handle's rights and retain READ_CONTROL for the
		// read-back check: cleanup must restore the exact protected DACL.
		h, err := windows.CreateFile(wide, requestedAccess, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE, nil, windows.OPEN_EXISTING, windows.FILE_FLAG_OPEN_REPARSE_POINT, 0)
		if err != nil {
			t.Errorf("open DACL restore handle (requested access %#x, original control %#x): %v", requestedAccess, originalControl, err)
			return
		}
		if err := windows.SetSecurityInfo(h, windows.SE_FILE_OBJECT, flags, nil, nil, acl, nil); err != nil {
			t.Errorf("restore original DACL (requested access %#x, original control %#x, security info %#x): %v", requestedAccess, originalControl, flags, err)
		} else {
			restored, err := windows.GetSecurityInfo(h, windows.SE_FILE_OBJECT, windows.DACL_SECURITY_INFORMATION)
			if err != nil {
				t.Errorf("read restored DACL (requested access %#x, original control %#x): %v", requestedAccess, originalControl, err)
			} else {
				originalUnchanged := bytes.Equal(originalStorage, unsafe.Slice((*byte)(unsafe.Pointer(original)), len(originalStorage)))
				if !originalUnchanged {
					t.Error("original DACL storage changed during restoration")
				}
				restoredACEs, copyErr := windowsTestCopyACEs(restored)
				if copyErr != nil {
					t.Errorf("snapshot restored ACEs: %v", copyErr)
				}
				restoredControl, _, err := restored.Control()
				if err != nil {
					t.Errorf("read restored DACL control: %v", err)
				} else {
					comparisonErr := windowsTestRestoredDACL(original, restored)
					if comparisonErr != nil || copyErr != nil || !originalUnchanged {
						t.Logf("DACL after restore: original storage unchanged=%v, original control=%#x, restored control=%#x", originalUnchanged, originalControl, restoredControl)
						for _, message := range windowsTestACEDifferences("after restore", originalACEs, restoredACEs) {
							t.Log(message)
						}
					}
					if comparisonErr != nil {
						t.Errorf("restored DACL mismatch (original control %#x, restored control %#x): %v", originalControl, restoredControl, comparisonErr)
					}
				}
			}
		}
		if err := windows.CloseHandle(h); err != nil {
			t.Errorf("close DACL restore handle: %v", err)
		}
	})
}

func TestWindowsDirectoryCrossProcessDACLRejectsNewOwner(t *testing.T) {
	root := t.TempDir()
	writer := windowsTestStartProcess(t, root, "writer")
	lock := filepath.Join(root, writer.ready.ControlName, writer.ready.LockName)
	writer.close(t)
	windowsTestBroadenDACL(t, lock)
	rejected := windowsTestStartProcess(t, root, "reject", "POLARIS_DIRECTORY_CHILD_REJECT_CONTAINS=unexpected access grants")
	rejected.wait(t, false)
}

func TestWindowsDirectoryCrossProcessOldLeasePoisonsOnDACLChange(t *testing.T) {
	root := t.TempDir()
	writer := windowsTestStartProcess(t, root, "writer", "POLARIS_DIRECTORY_CHILD_EXPECT_CLOSE_ERROR=1")
	windowsTestBroadenDACL(t, filepath.Join(root, writer.ready.ControlName, writer.ready.LockName))
	writer.close(t)
	rejected := windowsTestStartProcess(t, root, "reject", "POLARIS_DIRECTORY_CHILD_REJECT_CONTAINS=permanently revoked")
	rejected.wait(t, false)
}

// Artifact SHA256 is verified by Rust/PowerShell before this exact entry. Ready
// requires actual production activation and native acquisition, not exit code.
func TestWindowsDirectoryInteropChild(t *testing.T) {
	root, nonce := os.Getenv("POLARIS_DIRECTORY_INTEROP_ROOT"), os.Getenv("POLARIS_DIRECTORY_INTEROP_NONCE")
	if root == "" {
		t.Skip("Rust entry requires explicit root and nonce")
	}
	if !validDirectoryNonce(nonce) {
		t.Fatal("invalid interop nonce")
	}
	info, err := os.Stat(root)
	if err != nil || !info.IsDir() {
		t.Fatal("interop root must exist")
	}
	lease, err := windowsTestManagedWriter(root, "node")
	if err != nil {
		t.Fatal(err)
	}
	defer lease.releaseHandles()
	if err := windowsTestSignal(filepath.Join(root, "interop-ready"), nonce); err != nil {
		t.Fatal(err)
	}
	t.Log("WINDOWS_DIRECTORY_INTEROP_READY", nonce)
	deadline := time.NewTimer(60 * time.Second)
	defer deadline.Stop()
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	for {
		closeRequested, err := windowsTestHasSignal(filepath.Join(root, "interop-close"), nonce)
		if err != nil {
			t.Fatal(err)
		}
		if closeRequested {
			poison := os.Getenv("POLARIS_DIRECTORY_INTEROP_POISON") == "1"
			if poison {
				if err := PoisonDirectoryDomain(filepath.Join(root, "tailscale-windows-v1")); err != nil {
					t.Fatal(err)
				}
			}
			err := lease.Close()
			if (err != nil) != poison {
				t.Fatalf("unexpected interop Close: %v", err)
			}
			if lease.target != 0 || lease.lifetime != 0 {
				t.Fatal("interop Close retained uncertain custody")
			}
			if err := windowsTestSignal(filepath.Join(root, "interop-closed"), nonce); err != nil {
				t.Fatal(err)
			}
			return
		}
		select {
		case <-deadline.C:
			t.Fatal("interop close deadline expired")
		case <-ticker.C:
		}
	}
}
