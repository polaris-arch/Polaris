#!/usr/bin/env python3
"""Run a bounded native Go test binary in disposable Linux namespaces.

This never launches sing-box. The root guardian owns the deadline, process group,
pidfd and actual wait. --probe exercises isolation and timeout without networking.
"""
import argparse
import ctypes
import hashlib
import json
import math
import os
import re
from pathlib import Path
import signal
import shutil
import stat
import subprocess
import sys
import tempfile
import time

SCRIPT = Path(__file__).resolve()
PR_SET_CHILD_SUBREAPER = 36
PR_SET_PDEATHSIG = 1
NS_GET_NSTYPE = 0xB703
NS_GET_USERNS = 0xB701
CLONE_NEWNET = 0x40000000
NAMESPACE_NAMES = ("net", "mnt", "user", "pid", "ipc")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def identity(fd):
    value = os.fstat(fd)
    return {"dev": value.st_dev, "ino": value.st_ino}


def record(kind, **values):
    print(json.dumps({"kind": kind, **values}, sort_keys=True), flush=True)


def command(args, timeout=5):
    started = time.monotonic()
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout,
                            close_fds=True, check=False)
    return {"command": args, "status": result.returncode, "stdout": result.stdout,
            "stderr": result.stderr, "startedMonotonic": started,
            "finishedMonotonic": time.monotonic()}


def snapshot(deadline=None):
    return [command(args, timeout=5 if deadline is None else max(0.001, min(1, deadline-time.monotonic()))) for args in (
        ["/usr/sbin/ip", "-j", "link", "show"],
        ["/usr/sbin/ip", "-j", "address", "show"],
        ["/usr/sbin/ip", "-j", "-4", "route", "show", "table", "all"],
        ["/usr/sbin/ip", "-j", "-6", "route", "show", "table", "all"],
        ["/usr/sbin/ip", "-j", "-4", "rule", "show"],
        ["/usr/sbin/ip", "-j", "-6", "rule", "show"],
        ["/usr/sbin/nft", "--json", "list", "ruleset"],
        ["/usr/sbin/iptables-save"], ["/usr/sbin/ip6tables-save"],
    )]


def namespace_references(target):
    """Read-only final check for process or fd references to the new namespace."""
    references, errors = [], []
    for process in Path("/proc").iterdir():
        if not process.name.isdecimal():
            continue
        try:
            paths = [process / "ns/net", *(process / "fd").iterdir()]
        except FileNotFoundError:
            continue
        except OSError as error:
            errors.append({"pid": int(process.name), "error": str(error)})
            continue
        for path in paths:
            try:
                value = path.stat()
                if {"dev": value.st_dev, "ino": value.st_ino} == target:
                    references.append(str(path))
            except FileNotFoundError:
                pass
            except OSError as error:
                errors.append({"path": str(path), "error": str(error)})
    return {"references": references, "errors": errors}


def secure_directory(prefix):
    parent = Path("/var/tmp")
    value = parent.stat()
    require(value.st_uid == 0 and stat.S_ISDIR(value.st_mode)
            and not parent.is_symlink(), "unsafe evidence parent")
    directory = Path(tempfile.mkdtemp(prefix=prefix, dir=parent))
    value = directory.lstat()
    require(value.st_uid == 0 and stat.S_ISDIR(value.st_mode)
            and stat.S_IMODE(value.st_mode) == 0o700, "unsafe root evidence directory")
    return directory


def freeze_binary(source, expected, directory):
    require(len(expected) == 64 and all(c in "0123456789abcdef" for c in expected),
            "expected binary SHA256 is required")
    original = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    target = directory / "native.test"
    try:
        require(stat.S_ISREG(os.fstat(original).st_mode), "native binary is not regular")
        destination = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o500)
        digest = hashlib.sha256()
        try:
            while data := os.read(original, 1024 * 1024):
                digest.update(data)
                view = memoryview(data)
                while view:
                    view = view[os.write(destination, view):]
            os.fsync(destination)
        finally:
            os.close(destination)
        require(digest.hexdigest() == expected, "copied binary SHA256 differs")
        # Checking the protected copy, rather than a mutable input path, closes
        # verification/copy/exec substitution. Only this root-owned copy is used.
        metadata = command(["/usr/bin/file", str(target)])
        require(metadata["status"] == 0 and "ELF" in metadata["stdout"], "native test is not ELF")
        return target, {"sha256": digest.hexdigest(), "bytes": target.stat().st_size,
                        "file": metadata}
    finally:
        os.close(original)


def freeze_worker(directory):
    data = SCRIPT.read_bytes()
    target = directory / "worker.py"
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o500)
    try:
        with os.fdopen(fd, "wb", closefd=False) as output:
            output.write(data)
            output.flush()
            os.fsync(fd)
    finally:
        os.close(fd)
    return target, hashlib.sha256(data).hexdigest()


def parent_death(parent_pid):
    libc = ctypes.CDLL(None, use_errno=True)
    require(libc.prctl(PR_SET_PDEATHSIG, signal.SIGKILL, 0, 0, 0) == 0, "parent-death setup failed")
    if os.getppid() != parent_pid:
        os._exit(125)


def process_security():
    fields = {"CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb", "NoNewPrivs", "Seccomp", "Seccomp_filters"}
    return {key: value.strip() for line in Path("/proc/self/status").read_text().splitlines()
            if ":" in line for key, value in [line.split(":", 1)] if key in fields}


def private_root(evidence, binary, netfd, probe, r_context=None):
    """A closed root with no host socket/config paths or writable host mounts."""
    root = Path(evidence) / "root"
    root.mkdir(mode=0o700)
    require(command(["/usr/bin/mount", "-t", "tmpfs", "-o", "size=96m,mode=0700,nosuid,nodev", "tmpfs", str(root)])["status"] == 0, "private root tmpfs failed")
    for name in ("usr", "etc", "run", "var", "proc", "dev", "private"):
        (root / name).mkdir(mode=0o755 if name != "private" else 0o700)
    require(command(["/usr/bin/mount", "-t", "tmpfs", "-o", "size=1m,mode=0755,nosuid", "tmpfs", str(root / "dev")])["status"] == 0, "private devices tmpfs failed")
    for name in ("bin", "sbin", "lib", "lib64"):
        (root / name).symlink_to("usr/" + name)
    (root / "var/run").symlink_to("/run")
    (root / "tmp").symlink_to("/private/tmp")
    for name in ("guard", "receipts", "tmp"):
        (root / "private" / name).mkdir(mode=0o700)
    require(command(["/usr/bin/mount", "--bind", "/usr", str(root / "usr")])["status"] == 0, "tool bind failed")
    require(command(["/usr/bin/mount", "-o", "remount,bind,ro,nosuid,nodev", str(root / "usr")])["status"] == 0, "tool read-only bind failed")
    require(command(["/usr/bin/mount", "-t", "proc", "-o", "nosuid,nodev,noexec", "proc", str(root / "proc")])["status"] == 0, "private proc failed")
    # Child-userns root cannot mknod. Expose only two benign fixed devices;
    # no host directory, TUN, random-management ioctl or block device is visible.
    for name in ("null", "zero"):
        (root / "dev" / name).touch(mode=0o600)
        require(command(["/usr/bin/mount", "--bind", "/dev/" + name, str(root / "dev" / name)])["status"] == 0, "minimal device bind failed: " + name)
    (root / "dev/fd").symlink_to("/proc/self/fd")
    if not probe:
        shutil.copyfile(binary, root / "native.test")
        (root / "native.test").chmod(0o500)
        require(hashlib.sha256((root / "native.test").read_bytes()).hexdigest() == hashlib.sha256(Path(binary).read_bytes()).hexdigest(), "private binary copy differs")
    if r_context is not None:
        r_private_files(root, r_context)
    os.chroot(root)
    os.chdir("/private")
    if r_context is not None:
        r_setup(r_context)
    return filesystem_and_syscall_fence(netfd, r_context)


def filesystem_and_syscall_fence(netfd, r_context=None):
    """Keep setns for private netns only; deny obtaining other namespaces/root."""
    libc = ctypes.CDLL(None, use_errno=True)
    require(os.uname().machine in ("x86_64", "aarch64"), "unsupported fence syscall architecture")
    # Landlock ABI 3 adds truncation. Missing enforcement is a hard rejection.
    abi = libc.syscall(444, 0, 0, 1)
    require(abi >= 3, "Landlock ABI>=3 unavailable")
    writable = (1 << 1) | sum(1 << bit for bit in range(4, 15))
    ruleset_attribute = ctypes.c_uint64(writable)
    ruleset = libc.syscall(444, ctypes.byref(ruleset_attribute), ctypes.sizeof(ruleset_attribute), 0)
    require(ruleset >= 0, "Landlock ruleset failed")
    class PathRule(ctypes.Structure):
        _pack_ = 1
        _fields_ = [("allowed_access", ctypes.c_uint64), ("parent_fd", ctypes.c_int32)]
    try:
        paths = [("/private", writable), ("/dev/null", 1 << 1)]
        if r_context is not None:
            paths += [("/dev/net/tun", 1 << 1), ("/run/polaris-sing-tun-claims", writable)]
            paths += [("/r-metadata", (1 << 1) | (1 << 8))]
            # G setup restoration only. The native child stacks a ruleset excluding both leaves.
            paths += [(path, 1 << 1) for path in r_context["sysctls"]]
            if r_context["index"] == 16:
                paths.append(("/proc/sys/net/ipv4/conf/rnt16/rp_filter", 1 << 1))
        for path, rights in paths:
            fd = os.open(path, os.O_PATH | os.O_CLOEXEC)
            try:
                rule = PathRule(rights, fd)
                require(libc.syscall(445, ruleset, 1, ctypes.byref(rule), 0) == 0, "Landlock path rule failed")
            finally:
                os.close(fd)
        require(libc.prctl(38, 1, 0, 0, 0) == 0, "no_new_privs failed")
        require(libc.syscall(446, ruleset, 0) == 0, "Landlock enforcement failed")
    finally:
        os.close(ruleset)
    class CapHeader(ctypes.Structure):
        _fields_ = [("version", ctypes.c_uint32), ("pid", ctypes.c_int32)]
    class CapData(ctypes.Structure):
        _fields_ = [("effective", ctypes.c_uint32), ("permitted", ctypes.c_uint32), ("inheritable", ctypes.c_uint32)]
    capabilities = (CapData * 2)()
    header = CapHeader(0x20080522, 0)
    require(libc.capget(ctypes.byref(header), capabilities) == 0, "capget failed")
    keep = {12, 13, 21}  # NET_ADMIN/NET_RAW/SYS_ADMIN in the verified child userns.
    for capability in range(41):
        if capability not in keep:
            require(libc.prctl(24, capability, 0, 0, 0) == 0, "capability bounding drop failed")
    for word in range(2):
        mask = sum(1 << (bit % 32) for bit in keep if bit // 32 == word)
        capabilities[word].effective &= mask
        capabilities[word].permitted &= mask
        capabilities[word].inheritable = 0
    require(libc.capset(ctypes.byref(header), capabilities) == 0, "capset failed")
    seccomp = ctypes.CDLL("libseccomp.so.2", use_errno=True)
    seccomp.seccomp_init.restype = ctypes.c_void_p
    seccomp.seccomp_syscall_resolve_name.argtypes = [ctypes.c_char_p]
    seccomp.seccomp_rule_add_array.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int, ctypes.c_uint, ctypes.c_void_p]
    seccomp.seccomp_load.argtypes = [ctypes.c_void_p]
    seccomp.seccomp_release.argtypes = [ctypes.c_void_p]
    context = seccomp.seccomp_init(0x7FFF0000)  # SCMP_ACT_ALLOW
    require(context, "seccomp allocation failed")
    class ArgComparison(ctypes.Structure):
        _fields_ = [("argument", ctypes.c_uint), ("operation", ctypes.c_int),
                    ("value", ctypes.c_uint64), ("mask", ctypes.c_uint64)]
    try:
        def deny(name, comparison=None, error=1):
            number = seccomp.seccomp_syscall_resolve_name(name.encode())
            if number >= 0:
                require(seccomp.seccomp_rule_add_array(context, 0x00050000 | error, number,
                    1 if comparison is not None else 0, ctypes.byref(comparison) if comparison is not None else None) == 0, "seccomp rule failed: " + name)
        for name in ("mount", "umount2", "pivot_root", "chroot", "unshare", "open_by_handle_at", "ptrace", "process_vm_writev", "bpf", "init_module", "finit_module", "delete_module", "reboot", "kexec_load", "kexec_file_load", "swapon", "swapoff", "sethostname", "setdomainname", "fsopen", "fsmount", "fsconfig", "fspick", "open_tree", "move_mount", "mount_setattr"):
            deny(name)
        deny("clone3", error=38)  # ENOSYS permits pthread/Go's ordinary clone fallback.
        namespace_mask = 0x7E020000  # NEWNS/CGROUP/UTS/IPC/USER/PID/NET
        for bit in range(32):
            if namespace_mask & (1 << bit):
                deny("clone", ArgComparison(0, 7, 1 << bit, 1 << bit))
        deny("setns", ArgComparison(1, 1, CLONE_NEWNET, 0))  # only CLONE_NEWNET
        require(seccomp.seccomp_load(context) == 0, "seccomp enforcement failed")
    finally:
        seccomp.seccomp_release(context)
    security = process_security()
    mask = sum(1 << bit for bit in keep)
    require(int(security["CapEff"], 16) == mask and int(security["CapPrm"], 16) == mask
            and int(security["CapBnd"], 16) == mask and int(security["CapAmb"], 16) == 0
            and int(security["CapInh"], 16) == 0 and security["NoNewPrivs"] == "1"
            and security["Seccomp"] == "2", "actual capability/NNP/seccomp state differs")
    return {"landlockABI": abi, "capabilities": sorted(keep), "netnsSetnsOnly": True,
            "hostSocketsMasked": True, "hostFilesystemWritesDenied": True,
            "actualProcessSecurity": security, "minimalDevices": ["null", "zero"] + (["net/tun"] if r_context is not None else [])}


def reap_all(deadline):
    reaped = []
    while True:
        try:
            pid, status = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return reaped, True
        if pid:
            reaped.append({"pid": pid, "status": status})
            continue
        if time.monotonic() >= deadline:
            return reaped, False
        time.sleep(0.01)


def guardian(args):
    if getattr(args, "profile", N_PROFILE) == R_PROFILE:
        r_execution_gate(os.environ)
    require(os.geteuid() == 0, "guardian requires sudo/root")
    require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"), "pidfd API unavailable")
    require(1 <= args.deadline <= 120, "deadline must be 1..120 seconds")
    require(ctypes.CDLL(None, use_errno=True).prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) == 0,
            "subreaper setup failed")
    evidence = secure_directory("polaris-native-evidence-")
    frozen_worker, harness_hash = freeze_worker(evidence)
    def interrupted(signum, frame):
        raise InterruptedError("guardian interrupted by signal " + str(signum))
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    host_fds = [os.open("/proc/thread-self/ns/" + name, os.O_RDONLY | os.O_CLOEXEC) for name in NAMESPACE_NAMES]
    before_identity = {name: identity(fd) for name, fd in zip(NAMESPACE_NAMES, host_fds)}
    receipt = {"schema": "polaris-native-netns-v1", "hostBeforeIdentity": before_identity,
               "kernel": os.uname().release, "deadlineSeconds": args.deadline,
               "probe": args.probe, "candidate": None, "result": "Unknown"}
    receipt["harnessSha256"] = harness_hash
    receipt["profile"] = "nft-only-child-userns-v1"
    record("guardian-evidence", directory=str(evidence), harnessSha256=harness_hash)
    is_r = getattr(args, "profile", N_PROFILE) == R_PROFILE
    r_plan_value, r_verifier = None, None
    child, pidfd = None, None
    try:
        if is_r:
            receipt["profile"] = R_PROFILE
            receipt["hostClaimsBefore"] = r_host_claims_snapshot()
            r_plan_value, r_verifier, r_paths = r_inputs(args, evidence)
            args.expected_test = [r_verifier.CASES[args.r_case][0]]
            require(args.sha256 == r_plan_value["elfSha256"], "R exact candidate ELF differs")
        receipt["hostBefore"] = snapshot(time.monotonic()+10 if is_r else None)
        require(all(item["status"] == 0 for item in receipt["hostBefore"]), "host pre-snapshot failed")
        receipt["tools"] = [command([tool, "-Version" if tool.endswith("/ip") else "--version"]) for tool in
                            ("/usr/bin/unshare", "/usr/sbin/ip", "/usr/sbin/nft",
                             "/usr/sbin/iptables", "/usr/sbin/ip6tables")]
        binary = ""
        if not args.probe:
            require(args.binary and args.sha256 and args.expected_test, "binary/hash/expected-test required")
            require(len(args.expected_test) == len(set(args.expected_test)) and all(name.startswith("Test") and name.isidentifier() for name in args.expected_test), "invalid expected-test closed set")
            binary, receipt["candidate"] = freeze_binary(args.binary, args.sha256, evidence)
        worker = ["/usr/bin/unshare", "--user", "--map-root-user", "--ipc", "--net", "--mount", "--pid", "--fork",
                  "--kill-child=SIGKILL", "--mount-proc", "--propagation", "private",
                  "/usr/bin/python3", "-I", str(frozen_worker), "--worker", "--parent-net",
                  json.dumps(before_identity["net"]), "--parent-mnt",
                  json.dumps(before_identity["mnt"]), "--parent-identities",
                  json.dumps(before_identity), "--evidence", str(evidence),
                  "--deadline", str(args.deadline),
                  "--binary", str(binary)]
        for name in args.expected_test or []:
            worker += ["--expected-test", name]
        if args.probe:
            worker += ["--probe", args.probe]
        if is_r:
            worker += ["--profile", R_PROFILE, "--r-plan", r_paths["plan"], "--r-plan-sha256", args.r_plan_sha256, "--r-profile-json", r_paths["profile"], "--r-config-json", r_paths["config"], "--r-case", str(args.r_case)]
        receipt["workerArgv"] = worker
        guardian_pid = os.getpid()
        # No host namespace FD is inherited. The child only receives identity
        # numbers; the private proc mount exposes only its private PID namespace.
        with (evidence / "worker.ndjson").open("x") as output:
            child = subprocess.Popen(worker, stdin=subprocess.DEVNULL, stdout=output,
                                     stderr=output, start_new_session=True, close_fds=True,
                                     env={"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "PYTHONDONTWRITEBYTECODE": "1", **({"POLARIS_R_NATIVE_RUN": os.environ.get("POLARIS_R_NATIVE_RUN", ""), "POLARIS_NO_KERNEL_RUN": os.environ.get("POLARIS_NO_KERNEL_RUN", "")} if is_r else {})},
                                     preexec_fn=lambda: parent_death(guardian_pid))
            pidfd = os.pidfd_open(child.pid)
            receipt["guardianChild"] = {"pid": child.pid, "pgid": child.pid}
            record("guardian-child", pid=child.pid, pgid=child.pid)
            expires = time.monotonic() + args.deadline
            while child.poll() is None and time.monotonic() < expires:
                time.sleep(0.02)
            timed_out = child.poll() is None
            if timed_out:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                signal.pidfd_send_signal(pidfd, signal.SIGKILL)
            receipt["timedOut"] = timed_out
            receipt["childExit"] = child.wait(timeout=5)
            receipt["reaped"], receipt["allChildrenWaited"] = reap_all(time.monotonic() + 5)
        events = []
        worker_text = r_verifier.bounded_file(evidence / "worker.ndjson", 2*r_verifier.MAX_STDOUT+4194304).decode() if is_r else (evidence / "worker.ndjson").read_text()
        for line in worker_text.splitlines():
            try:
                event = json.loads(line)
                if isinstance(event, dict):
                    events.append(event)
            except ValueError:
                continue
        receipt["workerEvents"] = events
        namespace = next((event["net"] for event in events if event.get("kind") == "namespace"), None)
        receipt["namespaceReferences"] = namespace_references(namespace) if namespace else {"errors": ["namespace identity absent"], "references": []}
        after_fds = [os.open("/proc/thread-self/ns/" + name, os.O_RDONLY | os.O_CLOEXEC) for name in NAMESPACE_NAMES]
        try:
            receipt["hostAfterIdentity"] = {name: identity(fd) for name, fd in zip(NAMESPACE_NAMES, after_fds)}
        finally:
            for fd in after_fds:
                os.close(fd)
        receipt["hostAfter"] = snapshot(time.monotonic()+10 if is_r else None)
        if is_r:
            receipt["hostClaimsAfter"] = r_host_claims_snapshot()
        snapshots_valid = all(item["status"] == 0 for item in receipt["hostBefore"] + receipt["hostAfter"])
        # ip link statistics and nft counters can naturally change on the host;
        # retain raw evidence and require equality of configuration below.
        receipt["hostSnapshotsValid"] = snapshots_valid
        comparison = compare_snapshots(receipt["hostBefore"], receipt["hostAfter"]) if snapshots_valid else {"equal": False, "normalizations": [], "errors": ["snapshot command failed"]}
        receipt["hostConfigurationEqual"] = comparison["equal"]
        receipt["hostSnapshotComparison"] = comparison
        references = receipt["namespaceReferences"]
        receipt["controlledNamespaceLifetimeEnded"] = receipt["allChildrenWaited"] and not references["errors"] and not references["references"] and namespace is not None
        expected_exit = timed_out if args.probe == "timeout" else not timed_out and receipt["childExit"] == 0
        execution = next((event for event in events if event.get("kind") == "native-results"), None)
        native_verified = execution and execution.get("expected") == args.expected_test and execution.get("complete") is True
        safe = (receipt["controlledNamespaceLifetimeEnded"] and snapshots_valid and receipt["hostConfigurationEqual"]
                and receipt["hostAfterIdentity"] == before_identity)
        receipt["result"] = "ProbePass" if args.probe and expected_exit and safe else "NativeTestsPass" if not args.probe and expected_exit and safe and native_verified else "Unknown"
        if is_r:
            receipt["result"] = "Unknown"
            cases = [e for e in events if e.get("kind") == "r-case-result"]
            require(len(cases) == 1 and expected_exit and safe and receipt["hostClaimsBefore"] == receipt["hostClaimsAfter"], "R original worker/host settlement incomplete")
            envelope = cases[0]["envelope"]
            envelope["host"] = {"before": receipt["hostBefore"], "after": receipt["hostAfter"], "claimsBefore": receipt["hostClaimsBefore"], "claimsAfter": receipt["hostClaimsAfter"], "namespaceBefore": before_identity, "namespaceAfter": receipt["hostAfterIdentity"]}
            envelope["settlement"] = {"allChildrenWaited": receipt["allChildrenWaited"] and cases[0]["allChildrenWaited"], "controlledNamespaceLifetimeEnded": receipt["controlledNamespaceLifetimeEnded"], "cleanupWithinMillis": cases[0]["cleanupWithinMillis"]}
            receipt["rEnvelope"] = envelope
            receipt["result"] = "RCaseObservedConsumerNotReady" # Never full case0/28 functional PASS.

    except BaseException as error:
        receipt["error"] = f"{type(error).__name__}: {error}"
        if child is not None:
            if child.poll() is None:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                    if pidfd is not None:
                        signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            try:
                child.wait(timeout=5)
                receipt["reaped"], receipt["allChildrenWaited"] = reap_all(time.monotonic() + 5)
            except (subprocess.TimeoutExpired, OSError) as wait_error:
                receipt["waitError"] = str(wait_error)
    finally:
        if is_r:
            # Normal, timeout, cancellation and handled exception all retain actual host-after.
            # Guardian SIGKILL needs the original outer supervisor; absent evidence stays Unknown.
            try:
                if "hostAfter" not in receipt:
                    receipt["hostAfter"] = snapshot(time.monotonic()+10)
                if "hostClaimsAfter" not in receipt:
                    receipt["hostClaimsAfter"] = r_host_claims_snapshot()
                if "hostAfterIdentity" not in receipt:
                    refs = [os.open("/proc/thread-self/ns/"+name, os.O_RDONLY|os.O_CLOEXEC) for name in NAMESPACE_NAMES]
                    try:
                        receipt["hostAfterIdentity"] = {name:identity(fd) for name,fd in zip(NAMESPACE_NAMES,refs)}
                    finally:
                        for fd in refs: os.close(fd)
            except BaseException as after_error:
                receipt["hostAfterError"] = type(after_error).__name__
                receipt["result"] = "Unknown"
        if pidfd is not None:
            os.close(pidfd)
        for fd in host_fds:
            os.close(fd)
        (evidence / "receipt.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
        for path in evidence.iterdir():
            if path.is_file():
                path.chmod(0o644 if path.name != "native.test" else 0o500)
        evidence.chmod(0o755)
    print(json.dumps({"receipt": str(evidence / "receipt.json"), "result": receipt["result"]}), flush=True)
    return 0 if receipt["result"] in ("NativeTestsPass", "ProbePass") else 1


def compare_snapshots(before, after):
    """Normalize only known clocks/counters; retain all configuration structure."""
    normalizations, errors = [], []
    require(len(before) == len(after), "snapshot shape differs")
    equal = True
    for index, (old, new) in enumerate(zip(before, after)):
        if old["command"] != new["command"]:
            return {"equal": False, "normalizations": [], "errors": ["snapshot command differs"]}
        args, left, right = old["command"], old["stdout"], new["stdout"]
        if args[0].endswith("tables-save"):
            # Only the generated/completed timestamp comment is volatile.
            left = "\n".join(line for line in left.splitlines() if not line.startswith(("# Generated by ", "# Completed on ")))
            right = "\n".join(line for line in right.splitlines() if not line.startswith(("# Generated by ", "# Completed on ")))
        else:
            try:
                left, right = json.loads(left), json.loads(right)
            except ValueError:
                errors.append({"snapshot": index, "reason": "invalid JSON snapshot"})
                equal = False
                continue
            elapsed = new["startedMonotonic"] - old["startedMonotonic"]
            def clock_pair(a, b, field, path):
                if field not in a or field not in b or a[field] == b[field]:
                    return
                old_value, new_value = a[field], b[field]
                valid = (type(old_value) is int and type(new_value) is int and
                         0 <= new_value <= old_value < 4294967295 and
                         max(0, math.floor(elapsed)-2) <= old_value-new_value <= math.ceil(elapsed)+2)
                if valid:
                    normalizations.append({"snapshot": index, "path": path + "." + field,
                                           "before": old_value, "after": new_value,
                                           "elapsedSeconds": elapsed, "reason": "verified lifetime countdown"})
                    a[field] = b[field]
                else:
                    errors.append({"snapshot": index, "path": path + "." + field,
                                   "reason": "lifetime reset/renewal or unrecognized clock"})
            if args[0].endswith("/ip") and "address" in args and isinstance(left, list) and isinstance(right, list):
                for position, (a, b) in enumerate(zip(left, right)):
                    for address_index, (address_a, address_b) in enumerate(zip(a.get("addr_info", []), b.get("addr_info", []))):
                        for field in ("valid_life_time", "preferred_life_time"):
                            clock_pair(address_a, address_b, field, f"$[{position}].addr_info[{address_index}]")
            elif args[0].endswith("/ip") and "route" in args and isinstance(left, list) and isinstance(right, list):
                for position, (a, b) in enumerate(zip(left, right)):
                    clock_pair(a, b, "expires", f"$[{position}]")
            elif args[0].endswith("/nft"):
                def counter_pair(a, b, path):
                    if not isinstance(a, dict) or not isinstance(b, dict):
                        return
                    for field in ("packets", "bytes"):
                        if field not in a or field not in b or a[field] == b[field]:
                            continue
                        if type(a[field]) is int and type(b[field]) is int and 0 <= a[field] <= b[field]:
                            normalizations.append({"snapshot": index, "path": path + "." + field,
                                                   "before": a[field], "after": b[field], "reason": "monotonic nft counter"})
                            a[field] = b[field]
                        else:
                            errors.append({"snapshot": index, "path": path + "." + field, "reason": "counter reset or unrecognized value"})
                for position, (a, b) in enumerate(zip(left.get("nftables", []), right.get("nftables", []))):
                    if "counter" in a and "counter" in b:
                        counter_pair(a["counter"], b["counter"], f"$.nftables[{position}].counter")
                    if "rule" in a and "rule" in b:
                        for expr_index, (expr_a, expr_b) in enumerate(zip(a["rule"].get("expr", []), b["rule"].get("expr", []))):
                            if "counter" in expr_a and "counter" in expr_b:
                                counter_pair(expr_a["counter"], expr_b["counter"], f"$.nftables[{position}].rule.expr[{expr_index}].counter")
        equal = equal and left == right
    return {"equal": equal and not errors, "normalizations": normalizations, "errors": errors}


def native_results(stdout, expected, status, receipt_names):
    starts = [line.removeprefix("=== RUN   ") for line in stdout.splitlines() if line.startswith("=== RUN   ") and "/" not in line]
    finishes = {}
    for line in stdout.splitlines():
        match = re.match(r"^--- (PASS|FAIL|SKIP): (\w+) \(", line)
        if match:
            finishes.setdefault(match.group(2), []).append(match.group(1))
    complete = (status == 0 and sorted(starts) == sorted(expected)
                and set(finishes) == set(expected)
                and all(finishes[name] == ["PASS"] for name in expected)
                and all(name + ".ndjson" in receipt_names for name in expected)
                and len(receipt_names) <= 64)
    return {"expected": expected, "starts": starts, "finishes": finishes,
            "complete": complete, "receiptCount": len(receipt_names)}


def worker(args):
    if getattr(args, "profile", N_PROFILE) == R_PROFILE:
        r_execution_gate(os.environ)
    import fcntl
    netfd = os.open("/proc/thread-self/ns/net", os.O_RDONLY | os.O_CLOEXEC)
    mntfd = os.open("/proc/thread-self/ns/mnt", os.O_RDONLY | os.O_CLOEXEC)
    additional_fds = {name: os.open("/proc/thread-self/ns/" + name, os.O_RDONLY | os.O_CLOEXEC) for name in ("user", "pid", "ipc")}
    try:
        net, mnt = identity(netfd), identity(mntfd)
        require(os.geteuid() == 0 and os.getpid() == 1, "worker is not private PID namespace init")
        require(net != json.loads(args.parent_net) and mnt != json.loads(args.parent_mnt), "namespace isolation failed")
        require(fcntl.ioctl(netfd, NS_GET_NSTYPE) == CLONE_NEWNET, "fd is not a network namespace")
        current = {"net": net, "mnt": mnt, **{name: identity(fd) for name, fd in additional_fds.items()}}
        parent = json.loads(args.parent_identities)
        require(all(current[name] != parent[name] for name in NAMESPACE_NAMES), "child namespace equals guardian namespace")
        owners = {}
        for name, fd in {"net": netfd, "mnt": mntfd, "pid": additional_fds["pid"], "ipc": additional_fds["ipc"]}.items():
            ownerfd = fcntl.ioctl(fd, NS_GET_USERNS)
            try:
                owners[name] = identity(ownerfd)
                require(owners[name] == current["user"], "namespace not owned by child userns: " + name)
            finally:
                os.close(ownerfd)
        libc = ctypes.CDLL(None, use_errno=True)
        pdeath_before = ctypes.c_int()
        require(libc.prctl(2, ctypes.byref(pdeath_before), 0, 0, 0) == 0, "read post-mapping parent-death signal failed")
        # Rebuild at the final credential boundary. PID1's parent is outside its
        # private PID namespace; the live outer owner verifies the complete
        # guardian/unshare/PID1 chain by TERM/KILL probes and actual waits.
        parent_death(os.getppid())
        pdeath_after = ctypes.c_int()
        require(libc.prctl(2, ctypes.byref(pdeath_after), 0, 0, 0) == 0 and pdeath_after.value == signal.SIGKILL, "parent-death rebuild failed")
        record("namespace", net=net, mnt=mnt, processPID=os.getpid(), identities=current,
               owningUserNamespaces=owners, uidMap=Path("/proc/self/uid_map").read_text(),
               gidMap=Path("/proc/self/gid_map").read_text(), setgroups=Path("/proc/self/setgroups").read_text(),
               parentDeathBefore=pdeath_before.value, parentDeathAfter=pdeath_after.value,
               processSecurityBefore=process_security())
        if getattr(args, "profile", N_PROFILE) == R_PROFILE:
            return r_worker_body(args, netfd, mntfd, additional_fds, parent, current)
        for fd in additional_fds.values():
            os.close(fd)
        additional_fds.clear()
        fence = private_root(args.evidence, args.binary, netfd, args.probe)
        record("filesystem-profile", **fence)
        require(not Path("/run/dbus/system_bus_socket").exists() and not Path("/var/run/docker.sock").exists(), "host socket visible")
        require(not Path("/home/sway").exists() and not Path("/run/polaris-sing-tun-claims").exists(), "host source/claim path visible")
        require(all(not Path(path).exists() for path in ("/dev/net/tun", "/dev/random", "/dev/urandom")), "unneeded device visible")
        try:
            Path("/etc/isolation-write-probe").write_text("forbidden")
        except PermissionError:
            pass
        else:
            raise RuntimeError("system config write was allowed")
        record("host-filesystem-negative-controls", systemWriteDenied=True, defaultSocketsAbsent=True, originalRootAbsent=True)
        # The same privileged process used by the ELF cannot remount/re-root,
        # enter a mount namespace or obtain a new namespace after the fence.
        require(libc.setns(netfd, CLONE_NEWNET) == 0, "private netns setns positive control failed")
        currentfd = os.open("/proc/thread-self/ns/net", os.O_RDONLY | os.O_CLOEXEC)
        try:
            require(identity(currentfd) == net, "private setns changed namespace identity")
            record("private-setns-positive-control", success=True, namespace=identity(currentfd), syscallFlag=CLONE_NEWNET)
        finally:
            os.close(currentfd)
        require(libc.mount(b"none", b"/private/tmp", b"tmpfs", 0, None) == -1 and ctypes.get_errno() == 1, "mount escape was not denied")
        require(libc.unshare(0x20000) == -1 and ctypes.get_errno() == 1, "mount namespace escape was not denied")
        require(libc.setns(netfd, 0) == -1 and ctypes.get_errno() == 1, "unrestricted setns was not denied")
        record("syscall-negative-controls", mountDenied=True, unshareDenied=True, unrestrictedSetnsDenied=True)
        if args.probe:
            if args.probe == "timeout":
                descendant = os.fork()
                if descendant == 0:
                    os.setsid()
                    record("setsid-descendant", pid=os.getpid(), pgid=os.getpgrp())
                    time.sleep(300)
                    os._exit(0)
                record("timeout-ready", descendant=descendant)
                time.sleep(300)
            else:
                record("probe-complete")
            return 0
        private = Path("/private")
        guard, receipts = private / "guard", private / "receipts"
        if netfd != 3:
            os.dup2(netfd, 3)
        os.set_inheritable(3, True)
        environment = {"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "HOME": str(private),
                       "POLARIS_NATIVE_NAMESPACE_FD": "3",
                       "POLARIS_NATIVE_PARENT_NETNS_DEV": str(json.loads(args.parent_net)["dev"]),
                       "POLARIS_NATIVE_PARENT_NETNS_INO": str(json.loads(args.parent_net)["ino"]),
                       "POLARIS_NATIVE_GUARD_DIR": str(guard),
                       "POLARIS_NATIVE_RECEIPT_DIR": str(receipts),
                       "POLARIS_NATIVE_NONCE": os.urandom(12).hex(),
                       "POLARIS_NFT_KERNEL_TEST": "1", "POLARIS_NATIVE_FS_ISOLATED": "1",
                       "TMPDIR": "/private/tmp"}
        selector = "^(" + "|".join(args.expected_test) + ")$"
        listed = subprocess.run(["/native.test", "-test.list=" + selector], env=environment,
                                pass_fds=(3,), close_fds=True, capture_output=True, text=True, check=False)
        listed_names = [line for line in listed.stdout.splitlines() if line.startswith("Test")]
        require(listed.returncode == 0 and sorted(listed_names) == sorted(args.expected_test), "expected test list differs")
        record("native-test-list", expected=args.expected_test, actual=listed_names)
        result = subprocess.Popen(["/native.test", "-test.v", "-test.count=1", "-test.run=" + selector,
                                 "-test.timeout=" + str(max(1, args.deadline - 2)) + "s"],
                                env=environment, pass_fds=(3,), close_fds=True,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        output, output_bytes = [], 0
        for line in result.stdout:
            output_bytes += len(line.encode())
            require(output_bytes <= 16*1024*1024, "native output exceeds bound")
            print(line, end="", flush=True)
            output.append(line)
        result.wait()
        native_files = sorted(receipts.iterdir())
        require(sum(path.lstat().st_size for path in native_files) <= 16*1024*1024, "native journal total exceeds bound")
        verified = native_results("".join(output), args.expected_test, result.returncode, [path.name for path in native_files])
        record("native-results", **verified)
        record("native-exit", status=result.returncode, nonce=environment["POLARIS_NATIVE_NONCE"])
        # Capture only finite regular receipt files; do not traverse arbitrary
        # paths emitted by the native binary or retain namespace FDs.
        require(len(native_files) <= 64, "native receipt count exceeds limit")
        for path in native_files:
            metadata = path.lstat()
            require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= 8*1024*1024, "invalid native receipt")
            entries = [json.loads(line) for line in path.read_text().splitlines() if line]
            require(any(entry.get("phase") == "pending" for entry in entries) and any(entry.get("phase") == "complete" for entry in entries), "native transaction journal incomplete")
            record("native-receipt", name=path.name, sha256=hashlib.sha256(path.read_bytes()).hexdigest(), data=path.read_text())
        return result.returncode if verified["complete"] else 1
    finally:
        for fd in additional_fds.values():
            os.close(fd)
        os.close(mntfd)
        os.close(netfd)
        if netfd != 3 and not args.probe:
            os.close(3)


# Explicit R source branch in the original guardian/worker. This frozen source
# preparation does not issue native admission. A new reviewed source is required.
R_PROFILE = "native-tun-rtnetlink-child-userns-v1"
N_PROFILE = "nft-only-child-userns-v1"
R_SOURCE_EXECUTION_READY = False
R_METADATA_ACK = False
R_IOCTLS = (0xb703, 0xb701, 0x400454ca, 0x800454d2, 0x54e3, 0x8946)
# Actual frozen callers use route netlink and UDP/offload datagram sockets.
# No frozen R caller currently needs AF_UNIX, TCP, PACKET or NETFILTER.
R_SOCKET_TRIPLES = ((2,2,0),(2,2,17),(10,2,0),(10,2,17),(16,2,0),(16,3,0))
R_PLAN_FIELDS = frozenset("schema version profile harnessSha256 verifierSha256 profileSha256 configSha256 sourceCommit sourceTree sourceFilesSha256 moduleGraphSha256 elfSha256 sourceManifest moduleGraph goModSha256 goSumSha256 toolchain buildFlags batchNonce caseNonces cases selectedTopLists pcMetadataAck".split())


def r_module():
    import importlib.util
    from types import SimpleNamespace
    spec = importlib.util.spec_from_file_location("r_closed_receipts", SCRIPT.with_name("verify-r-native-receipts.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    # Use the actual loaded definitions after chroot; no host source reopen from FD8 validation.
    module._HARNESS = SimpleNamespace(r_options=r_options,r_topology=r_topology,r_plan=r_plan,compare_snapshots=compare_snapshots)
    return module


def r_execution_gate(environment):
    require(not environment.get("POLARIS_NO_KERNEL_RUN", ""), "NOT_EXECUTED: POLARIS_NO_KERNEL_RUN forbids R effects")
    require(environment.get("POLARIS_R_NATIVE_RUN") == "1", "NOT_EXECUTED: explicit R opt-in absent")
    require(R_SOURCE_EXECUTION_READY and R_METADATA_ACK, "NOT_EXECUTED: R source/PC ABI not reviewed and admitted")


def r_plan(value, verifier):
    verifier.exact(value, R_PLAN_FIELDS, "frozen R plan")
    require(value["schema"] == verifier.PLAN_SCHEMA and type(value["version"]) is int and value["version"] == 1 and value["profile"] == R_PROFILE, "R plan identity differs")
    for key in ("harnessSha256","verifierSha256","profileSha256","configSha256","sourceFilesSha256","moduleGraphSha256","elfSha256","goModSha256","goSumSha256"):
        require(type(value[key]) is str and verifier.HASH.fullmatch(value[key]), "R plan hash differs: " + key)
    for key in ("sourceCommit","sourceTree"):
        require(type(value[key]) is str and verifier.COMMIT.fullmatch(value[key]), "R source identity differs")
    require(type(value["batchNonce"]) is str and verifier.NONCE.fullmatch(value["batchNonce"]), "R batch nonce differs")
    require(type(value["caseNonces"]) is list and len(value["caseNonces"]) == 28 and len(set(value["caseNonces"])) == 28 and all(type(n) is str and verifier.NONCE.fullmatch(n) for n in value["caseNonces"]), "R case nonce inventory differs")
    require(type(value["cases"]) is list and len(value["cases"]) == 28, "R case inventory differs")
    for index, case in enumerate(value["cases"]):
        verifier.exact(case, {"selected","actors","options","topology","expectedSubjectCleanup","expectedFixtureDisposal"}, "R planned case")
        require(case["selected"] == "/".join(verifier.CASES[index]) and case["actors"] == (["a","b"] if index == 0 else ["single"]), "R closed selector/actors differs")
        require(type(case["options"]) is dict and set(case["options"]) == set(case["actors"]) and type(case["topology"]) is dict, "PC actual Options/topology absent")
        for actor in case["actors"]:
            r_options(case["options"][actor],index,actor)
        r_topology(case["topology"], index)
        require(case["expectedSubjectCleanup"] in ("Closed","ConstructionOnlyClosed","Unknown","PartialSealed") and case["expectedFixtureDisposal"] == "ActualRestoredAndDisposed", "PC expected disposition missing")
        if index == 14: require(case["expectedSubjectCleanup"] == "Unknown", "case14 Unknown must not be laundered")
        if index == 20: require(case["expectedSubjectCleanup"] == "PartialSealed", "case20 explicit partial seam must remain distinct")
    # These are producer raw-list observations tied to this exact ELF, never invented expected data.
    require(type(value["selectedTopLists"]) is list and len(value["selectedTopLists"]) == 5, "actual frozen selected-top list evidence missing")
    for item, (top, _) in zip(value["selectedTopLists"], verifier.GROUPS):
        verifier.exact(item,{"top","argv","stdout","exitStatus","elfSha256"},"producer top list")
        require(item["top"] == top and item["argv"] == ["-test.list=^"+top+"$"] and item["stdout"] == top+"\n" and type(item["exitStatus"]) is int and item["exitStatus"] == 0 and item["elfSha256"] == value["elfSha256"],"actual selected-top list differs")
    require(type(value["sourceManifest"]) is dict and value["sourceManifest"] and all(type(k) is str and type(v) is str and verifier.HASH.fullmatch(v) for k,v in value["sourceManifest"].items()) and hashlib.sha256(verifier.encoded(value["sourceManifest"],4194304)).hexdigest() == value["sourceFilesSha256"],"full frozen source manifest missing/differs")
    require(type(value["moduleGraph"]) is dict and value["moduleGraph"].get("complete") is True and type(value["moduleGraph"].get("modules")) is list and value["moduleGraph"]["modules"] and hashlib.sha256(verifier.encoded(value["moduleGraph"],4194304)).hexdigest() == value["moduleGraphSha256"],"complete actual module/replacement graph missing")
    require(type(value["toolchain"]) is str and value["toolchain"] and type(value["buildFlags"]) is list and "polaris_r_native" in value["buildFlags"],"actual producer toolchain/build flags missing")
    require(value["pcMetadataAck"] == "G_PC_EXACT_DICT_PENDING", "unissued PC ACK cannot activate this source")
    return value


def r_options(value,index,actor):
    # Exact transport dictionary proposed to the PC owner. No opaque callback/factory/path input.
    fields = {"name","netnsFD","providedTunFD","table","rulePriority","fallbackPriority","mtu","family","autoRoute","strict","marked","multiQueue","dnsMode","gso","txChecksumOffload","inet4Address","inet6Address","inputMark","outputMark","include","exclude","loopback","gateway","actualOptions","actualOptionsSha256"}
    require(type(value) is dict and set(value) == fields, "complete PC actor Options exact dictionary missing")
    expected_name = "rnt00b" if actor == "b" else "rnt%02d" % index
    require(value["name"] == expected_name and type(value["netnsFD"]) is int and value["netnsFD"] == 3 and (type(value["providedTunFD"]) is int and value["providedTunFD"]==9 if index==17 else value["providedTunFD"] is None), "Options namespace/name/FD differs")
    require(all(type(value[k]) is int for k in ("table","rulePriority","fallbackPriority")) and value["table"] == (40100 if actor == "b" else 40000+index) and value["rulePriority"] == 12000+64*index and value["fallbackPriority"] == 16000+64*index, "closed table/priority differs")
    require(type(value["mtu"]) is int and value["mtu"] == 1400 and value["dnsMode"] == "disabled" and value["gso"] is False and value["txChecksumOffload"] is False, "unreviewed R effect Options")
    require(type(value["family"]) is int and value["family"] == (6 if index in (4,24,25) else 4), "Options family differs")
    for key,want in (("autoRoute",index not in (23,25)),("strict",index==6),("marked",index==7),("multiQueue",index==18)):
        require(type(value[key]) is bool and value[key] is want,"Options branch differs: "+key)
    require(value["inet4Address"] == ([] if value["family"] == 6 else ["198.18.0.1/24"]) and value["inet6Address"] == (["fd00:727::1/64"] if value["family"] == 6 else []) and type(value["inputMark"]) is int and type(value["outputMark"]) is int and value["inputMark"] == 0x210001 and value["outputMark"] == 0x210002, "source fixed address/marks differ")
    import ipaddress
    for key in ("include","exclude","loopback"):
        require(type(value[key]) is list and len(value[key]) <= 4,"Options prefix budget")
        for text in value[key]:
            network = ipaddress.ip_network(text,strict=False) if key != "loopback" else ipaddress.ip_network(text+"/32",strict=False)
            require(network.subnet_of(ipaddress.ip_network("198.18.0.0/15")),"Options endpoint outside fixed private topology")
    require(value["gateway"] is None or str(ipaddress.ip_address(value["gateway"])) == value["gateway"],"gateway encoding differs")
    require(all(value[key]==[] for key in ("include","exclude","loopback")) and value["gateway"] is None,"constructor Options differ from frozen source; updates belong in actual operation evidence")
    require(type(value["actualOptionsSha256"]) is str and re.fullmatch("[0-9a-f]{64}",value["actualOptionsSha256"]),"actual PC Options source digest missing")
    # PC must supply its full actual constructor Options, including zero defaults.
    # G binds the original canonical body; it cannot invent the Go field projection.
    require(type(value["actualOptions"]) is dict and value["actualOptions"],"full actual PC Options body missing")
    actual=json.dumps(value["actualOptions"],sort_keys=True,separators=(",",":"),allow_nan=False).encode()
    require(len(actual)<=32768 and hashlib.sha256(actual).hexdigest()==value["actualOptionsSha256"],"full actual PC Options body/digest differs")


def r_topology(value,index):
    require(type(value) is dict and set(value) == {"persistentBefore","providedEndpoint","foreignLink","gatewayLink","ipv6OutputPriority","packetBudgetAllocation"}, "PC topology exact dictionary missing")
    persistent=value["persistentBefore"]
    require((persistent is not None) == (index in (15,16,17)),"persistent topology case differs")
    if persistent is not None:
        require(type(persistent) is dict and set(persistent) == {"name","mtu","up","addresses"} and persistent["name"] == "rnt%02d"%index and type(persistent["mtu"]) is int and 576<=persistent["mtu"]<=9000 and type(persistent["up"]) is bool and type(persistent["addresses"]) is list and 1<=len(persistent["addresses"])<=4,"actual persistent original snapshot fields missing")
        require(index != 16 or persistent["mtu"] != 1400 or persistent["up"] is False,"case16 must exercise actual borrowed change")
        require(index != 17 or persistent["up"] is True,"provided queue must start UP")
    for key in ("foreignLink","gatewayLink"):
        item=value[key]
        require((item is not None) == (index in ((11,12,13) if key=="foreignLink" else (19,))),"private foreign/gateway topology case differs")
        if item is not None:
            require(type(item) is dict and set(item)=={"name","kind","mtu","up","addresses","route"} and re.fullmatch(r"rfg[0-9]{2}",item["name"]) and item["kind"]=="dummy" and type(item["mtu"]) is int and 576<=item["mtu"]<=9000 and type(item["up"]) is bool and type(item["addresses"]) is list and len(item["addresses"])<=4,"PC private link exact fields missing")
            require(item["name"]=="rfg%02d"%index,"private link case identity differs")
            route=item["route"]
            require(type(route) is dict and set(route)=={"destination","gateway","table","metric"} and type(route["table"]) is int and 40000<=route["table"]<=40100 and type(route["metric"]) is int and 0<=route["metric"]<=65535,"PC full route dictionary missing")
            import ipaddress
            require(ipaddress.ip_network(route["destination"],strict=False).subnet_of(ipaddress.ip_network("198.18.0.0/15")) and ipaddress.ip_address(route["gateway"]) in ipaddress.ip_network("198.18.0.0/15"),"private route outside fixed topology")
    import ipaddress
    for item in (persistent,value["foreignLink"],value["gatewayLink"]):
        if item is not None:
            for address in item["addresses"]:
                require(type(address) is str and ipaddress.ip_interface(address).network.subnet_of(ipaddress.ip_network("198.18.0.0/15")),"prebuilt address outside fixed topology")
    require((value["providedEndpoint"] is not None)==(index==17),"case17 UP/address/endpoint route missing")
    if index==17:require(value["providedEndpoint"]=={"destination":"198.18.99.10/32","udpPort":19001,"linkName":"rnt17"},"case17 endpoint topology differs")
    require(type(value["ipv6OutputPriority"]) is int and value["ipv6OutputPriority"]>0 if index==25 else value["ipv6OutputPriority"] is None,"actual IPv6 autoRoute=false output priority missing")
    require(type(value["packetBudgetAllocation"]) is dict and set(value["packetBudgetAllocation"])=={"native","guardian"} and all(type(n) is int and 0<=n<=8 for n in value["packetBudgetAllocation"].values()) and sum(value["packetBudgetAllocation"].values())<=8,"shared total packet budget missing")


def r_child_environment(environment,metadata):
    # Admission runs before a whitelist can erase the caller's deny marker.
    require(not environment.get("POLARIS_NO_KERNEL_RUN", ""), "R deny marker cannot be washed out")
    return {"PATH":"/usr/sbin:/usr/bin:/sbin:/bin","HOME":"/private","TMPDIR":"/private/tmp",
            "POLARIS_R_NATIVE_RUN":"1","POLARIS_NATIVE_FS_ISOLATED":"1","POLARIS_R_NATIVE_METADATA_FD":"8"}


def r_freeze_file(source,expected,directory,name,maximum):
    require(re.fullmatch("[0-9a-f]{64}",expected) is not None,"frozen source hash missing")
    fd=os.open(source,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
    target=Path(directory)/name
    try:
        before=os.fstat(fd);require(stat.S_ISREG(before.st_mode) and 0<before.st_size<=maximum,"frozen R input type/size")
        data=bytearray()
        while chunk:=os.read(fd,min(65536,maximum+1-len(data))):
            data.extend(chunk);require(len(data)<=maximum,"R input grew beyond byte budget")
        after=os.fstat(fd);require((before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns) and len(data)==before.st_size,"R input changed while copying")
        require(hashlib.sha256(data).hexdigest()==expected,"R protected copy hash differs")
        out=os.open(target,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,0o444)
        try:
            view=memoryview(data)
            while view:view=view[os.write(out,view):]
            os.fsync(out)
        finally:os.close(out)
        return target,bytes(data)
    finally:os.close(fd)


def r_effect_program(architecture):
    """Finite classic BPF: existing deny filter remains; close R socket/ioctl effects only."""
    numbers={"x86_64":(0xc000003e,41,16),"aarch64":(0xc00000b7,198,29)}
    require(architecture in numbers,"unsupported R effect architecture")
    arch,socket_nr,ioctl_nr=numbers[architecture];instructions=[];labels={}
    def emit(code,k=0,jt=0,jf=0):instructions.append([code,jt,jf,k])
    def label(name):labels[name]=len(instructions)
    def eq(k,yes,no):emit(0x15,k,yes,no)
    def load(offset):emit(0x20,offset)
    # Jump destinations are resolved only for this finite program, never caller-generated code.
    load(4);eq(arch,"syscall","kill");label("kill");emit(0x06,0x80000000)
    label("syscall");load(0)
    if architecture=="x86_64":
        emit(0x54,0x40000000);eq(0,"native-abi","deny");label("native-abi");load(0)
    eq(socket_nr,"socket","ioctl-test")
    label("ioctl-test");eq(ioctl_nr,"ioctl","allow")
    label("socket")
    for arg in range(3):load(16+8*arg+4);eq(0,"socket-high-"+str(arg),"deny");label("socket-high-"+str(arg))
    for number,(family,kind,protocol) in enumerate(R_SOCKET_TRIPLES):
        next_label="socket-next-"+str(number);load(16);eq(family,"socket-type-"+str(number),next_label)
        label("socket-type-"+str(number));load(24);emit(0x54,0xffffffff ^ (0x80000|0x800));eq(kind,"socket-proto-"+str(number),next_label)
        label("socket-proto-"+str(number));load(32);eq(protocol,"allow",next_label);label(next_label)
    emit(0x05,"deny")
    label("ioctl");load(28);eq(0,"ioctl-low","deny");label("ioctl-low");load(24)
    for number,request in enumerate(R_IOCTLS):eq(request,"allow","ioctl-next-"+str(number));label("ioctl-next-"+str(number))
    label("deny");emit(0x06,0x00050001)
    label("allow");emit(0x06,0x7fff0000)
    for position,item in enumerate(instructions):
        if item[0]==0x05 and type(item[3]) is str:item[3]=labels[item[3]]-position-1
        for offset in (1,2):
            if type(item[offset]) is str:
                item[offset]=labels[item[offset]]-position-1;require(0<=item[offset]<=255,"R BPF jump out of range")
    return tuple(tuple(item) for item in instructions)


def r_install_effect_filter():
    class Filter(ctypes.Structure):_fields_=[("code",ctypes.c_ushort),("jt",ctypes.c_ubyte),("jf",ctypes.c_ubyte),("k",ctypes.c_uint)]
    class Program(ctypes.Structure):_fields_=[("length",ctypes.c_ushort),("filter",ctypes.POINTER(Filter))]
    body=r_effect_program(os.uname().machine);entries=(Filter*len(body))(*(Filter(code,jt,jf,k) for code,jt,jf,k in body));program=Program(len(body),entries)
    libc=ctypes.CDLL(None,use_errno=True)
    require(libc.prctl(22,2,ctypes.byref(program),0,0)==0,"R effect filter failed")


def r_native_landlock(index):
    """Stack a child-only narrow ruleset; G's two setup restore leaves are not inherited writable."""
    libc=ctypes.CDLL(None,use_errno=True);writable=(1<<1)|sum(1<<bit for bit in range(4,15));attr=ctypes.c_uint64(writable)
    ruleset=libc.syscall(444,ctypes.byref(attr),ctypes.sizeof(attr),0);require(ruleset>=0,"R child Landlock creation failed")
    class Rule(ctypes.Structure):
        _pack_=1;_fields_=[("allowed_access",ctypes.c_uint64),("parent_fd",ctypes.c_int32)]
    paths=[("/private",writable),("/dev/null",1<<1),("/dev/net/tun",1<<1),("/run/polaris-sing-tun-claims",writable)]
    if index==16:paths.append(("/proc/sys/net/ipv4/conf/rnt16/rp_filter",1<<1))
    try:
        for path,rights in paths:
            fd=os.open(path,os.O_PATH|os.O_NOFOLLOW|os.O_CLOEXEC)
            try:rule=Rule(rights,fd);require(libc.syscall(445,ruleset,1,ctypes.byref(rule),0)==0,"R child narrow path rule failed")
            finally:os.close(fd)
        require(libc.syscall(446,ruleset,0)==0,"R child Landlock enforcement failed")
    finally:os.close(ruleset)


def r_spawn(binary,argv,mapping,index,actor,environment,children):
    """Reorder in this single original worker before Popen; native pass_fds is only final slots."""
    import fcntl
    require(type(index) is int and 0<=index<=27 and actor in (("a","b") if index==0 else ("single",)),"R actor/case differs")
    slots=tuple(range(3,9))+((9,) if index==17 else ())+((10,) if index==0 and actor=="a" else ())
    require(tuple(sorted(mapping)) == slots,"R inherited FD closure differs")
    high={};saved={};child=None
    try:
        # Duplicate every source first. Save the original worker slots, including holes, then restore.
        for slot,source in mapping.items():high[slot]=fcntl.fcntl(source,fcntl.F_DUPFD_CLOEXEC,32)
        for slot in mapping:
            try:saved[slot]=fcntl.fcntl(slot,fcntl.F_DUPFD_CLOEXEC,32)
            except OSError as error:
                import errno
                require(error.errno==errno.EBADF,"R worker slot save failed");saved[slot]=None
        for slot in mapping:os.dup2(high[slot],slot,inheritable=True)
        parent=os.getpid()
        def child_fence():
            parent_death(parent);r_native_landlock(index);r_install_effect_filter()
        child=subprocess.Popen([binary,*argv],stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=environment,pass_fds=tuple(sorted(mapping)),close_fds=True,start_new_session=True,preexec_fn=child_fence)
        children.append(child)
        return child
    finally:
        for slot,source in saved.items():
            if source is None:
                try:os.close(slot)
                except OSError:pass
            else:os.dup2(source,slot,inheritable=False)
        for fd in list(high.values())+[fd for fd in saved.values() if fd is not None]:os.close(fd)


def r_host_claims_snapshot():
    """Outer only, bounded NOFOLLOW reads; never create/flock/unlink host claims or pass this FD."""
    base=Path("/run/polaris-sing-tun-claims")
    try:fd=os.open(base,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC)
    except FileNotFoundError:return {"absent":True}
    try:
        value=os.fstat(fd);require(stat.S_ISDIR(value.st_mode),"host claims base is not a directory")
        names=sorted(os.listdir(fd));require(len(names)<=4096,"host claims entry budget")
        total=0;entries=[]
        for name in names:
            held=os.open(name,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK|os.O_CLOEXEC,dir_fd=fd)
            try:
                metadata=os.fstat(held);require(stat.S_ISREG(metadata.st_mode) and metadata.st_size<=4194304-total,"host claims regular/total bound")
                data=bytearray()
                while chunk:=os.read(held,min(65536,4194305-total-len(data))):
                    data.extend(chunk);require(total+len(data)<=4194304,"host claims grew over byte budget")
                after=os.fstat(held);require((metadata.st_dev,metadata.st_ino,metadata.st_size,metadata.st_mtime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns) and len(data)==metadata.st_size,"host claim changed while reading")
                total+=len(data);entries.append({"name":name,"dev":metadata.st_dev,"ino":metadata.st_ino,"uid":metadata.st_uid,"gid":metadata.st_gid,"mode":stat.S_IMODE(metadata.st_mode),"nlink":metadata.st_nlink,"mtimeNs":metadata.st_mtime_ns,"ctimeNs":metadata.st_ctime_ns,"sha256":hashlib.sha256(data).hexdigest()})
            finally:os.close(held)
        return {"absent":False,"identity":identity(fd),"uid":value.st_uid,"gid":value.st_gid,"mode":stat.S_IMODE(value.st_mode),"entries":entries}
    finally:os.close(fd)



def r_inputs(args,directory):
    verifier=r_module()
    frozen,data=r_freeze_file(args.r_plan,args.r_plan_sha256,directory,"r-plan.json",4194304)
    plan=r_plan(verifier.closed_json(data,4194304),verifier)
    require(hashlib.sha256(SCRIPT.read_bytes()).hexdigest()==plan["harnessSha256"],"R actual frozen G source hash differs")
    r_freeze_file(SCRIPT.with_name("verify-r-native-receipts.py"),plan["verifierSha256"],directory,"verify-r-native-receipts.py",262144)
    profile_path,profile_data=r_freeze_file(args.r_profile_json,plan["profileSha256"],directory,"r-profile.json",65536)
    config_path,config_data=r_freeze_file(args.r_config_json,plan["configSha256"],directory,"r-config.json",65536)
    profile=verifier.closed_json(profile_data);config=verifier.closed_json(config_data)
    require(type(profile) is dict and profile.get("profile")==R_PROFILE and [(c.get("top"),c.get("sub")) for c in profile.get("cases",[])]==list(verifier.CASES),"PC exact frozen profile inventory differs")
    require(config=={"schema":"polaris-g-r-actor-config-v1","version":1,"cases":plan["cases"]},"PC config/actor exact dictionary differs")
    return plan,verifier,{"plan":str(frozen),"profile":str(profile_path),"config":str(config_path)}


def r_private_files(root,context):
    # Only a single validated char device is exposed; no directory bind of host /dev/net.
    value=os.stat("/dev/net/tun",follow_symlinks=False)
    require(stat.S_ISCHR(value.st_mode) and os.major(value.st_rdev)==10 and os.minor(value.st_rdev)==200,"actual TUN char device identity differs")
    (root/"dev/net").mkdir(mode=0o755);(root/"dev/net/tun").touch(mode=0o600)
    require(command(["/usr/bin/mount","--bind","/dev/net/tun",str(root/"dev/net/tun")])["status"]==0,"single private TUN bind failed")
    claims=root/"run/polaris-sing-tun-claims";claims.mkdir(mode=0o1777);claims.chmod(0o1777)
    protocol=b'{"protocol":"polaris-sing-tun-claims","version":2,"layout":"sticky-flat","allocator":".allocator"}\n'
    for name,data in ((".protocol",protocol),(".allocator",b"")):
        fd=os.open(claims/name,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,0o444)
        try:
            if data:require(os.write(fd,data)==len(data),"private claims marker short write")
            os.fsync(fd)
        finally:os.close(fd)
    (root/"r-metadata").mkdir(mode=0o700)


def r_link_observation(name):
    item=command(["/usr/sbin/ip","-j","-d","link","show","dev",name],timeout=1)
    require(item["status"]==0,"actual private link observation failed")
    values=json.loads(item["stdout"]);require(len(values)==1 and values[0].get("ifname")==name and type(values[0].get("ifindex")) is int,"actual name/index link binding differs")
    addresses=command(["/usr/sbin/ip","-j","address","show","dev",name],timeout=1)
    require(addresses["status"]==0,"private address observation failed")
    return {"link":values[0],"addresses":json.loads(addresses["stdout"])}


def r_link_configuration(observed):
    # Preserve every address selector; link traffic counters are not restoration state.
    link=observed["link"]
    require(type(link.get("ifindex")) is int and type(link.get("mtu")) is int and type(link.get("flags")) is list,"actual link configuration missing")
    addresses=observed["addresses"]
    require(type(addresses) is list and len(addresses)==1 and addresses[0].get("ifindex")==link["ifindex"],"actual address identity differs")
    return {"ifindex":link["ifindex"],"name":link["ifname"],"mtu":link["mtu"],"up":"UP" in link["flags"],"addresses":addresses[0].get("addr_info",[])}


def r_ip(*arguments):
    result=command(["/usr/sbin/ip",*arguments],timeout=1)
    require(result["status"]==0,"fixed private topology command failed")
    return result


def r_setup(context):
    import fcntl,struct,ipaddress
    index=context["index"];case=context["plan"]["cases"][index];context["setupObjects"]=[];context["providedTun"]=None;context["sysctls"]={}
    for leaf in ("all","default"):
        path="/proc/sys/net/ipv4/conf/"+leaf+"/rp_filter";original=Path(path).read_bytes();require(original in (b"0\n",b"1\n",b"2\n"),"private setup rp_filter invalid")
        context["sysctls"][path]={"bytes":original,"identity":Path(path).stat().st_ino};Path(path).write_bytes(b"0\n")
    persistent=case["topology"]["persistentBefore"]
    if persistent is not None:
        fd=os.open("/dev/net/tun",os.O_RDWR|os.O_NONBLOCK|os.O_CLOEXEC)
        try:
            request=struct.pack("16sH22x",persistent["name"].encode(),0x0001|0x1000)
            actual=fcntl.ioctl(fd,0x400454ca,request);require(actual[:16].split(b"\0",1)[0].decode()==persistent["name"],"actual private persistent TUN name differs")
            fcntl.ioctl(fd,0x400454cb,1)
            observed=r_link_observation(persistent["name"])
            context["setupObjects"].append({"name":persistent["name"],"kind":"tun","ifindex":observed["link"]["ifindex"]})
            r_ip("link","set","dev",persistent["name"],"mtu",str(persistent["mtu"]))
            for address in persistent["addresses"]:
                interface=ipaddress.ip_interface(address);require(interface.ip.is_private,"PC topology address is not private")
                r_ip("address","add",str(interface),"dev",persistent["name"])
            r_ip("link","set","dev",persistent["name"],"up" if persistent["up"] else "down")
            if index==17:
                context["providedTun"]=fd;fd=None
                r_ip("route","add","198.18.99.10/32","dev",persistent["name"])
                context["providedTunIdentity"]=identity(context["providedTun"])
            context["persistentBeforeObserved"]=r_link_observation(persistent["name"])
            # 15/16 intentionally close creator queue before R attaches. Never add multi-queue.
        finally:
            if fd is not None:os.close(fd)
    for key in ("foreignLink","gatewayLink"):
        item=case["topology"][key]
        if item is None:continue
        r_ip("link","add","name",item["name"],"type","dummy")
        observed=r_link_observation(item["name"])
        context["setupObjects"].append({"name":item["name"],"kind":"dummy","ifindex":observed["link"]["ifindex"]})
        r_ip("link","set","dev",item["name"],"mtu",str(item["mtu"]))
        for address in item["addresses"]:
            interface=ipaddress.ip_interface(address);require(interface.ip.is_private,"PC private link address differs")
            r_ip("address","add",str(interface),"dev",item["name"])
        r_ip("link","set","dev",item["name"],"up" if item["up"] else "down")
        route=item["route"]
        require(type(route) is dict and set(route)=={"destination","gateway","table","metric"},"PC route exact fields missing")
        network=ipaddress.ip_network(route["destination"],strict=False);gateway=ipaddress.ip_address(route["gateway"])
        require(network.is_private and gateway.is_private and type(route["table"]) is int and 40000<=route["table"]<=40100 and type(route["metric"]) is int and 0<=route["metric"]<=65535,"PC topology route exceeds closure")
        r_ip("route","add",str(network),"via",str(gateway),"dev",item["name"],"table",str(route["table"]),"metric",str(route["metric"]))
        context.setdefault("topologyObserved",{})[key]=r_link_observation(item["name"])
    if index==16:
        leaf="/proc/sys/net/ipv4/conf/rnt16/rp_filter";Path(leaf).write_bytes(b"1\n")
        context["writerBefore"]={"path":leaf,"identity":Path(leaf).stat().st_ino,"value":Path(leaf).read_bytes().decode()}
        require(context["writerBefore"]["value"]=="1\n","actual private writer seed differs")
    context["claimsFD"]=os.open("/run/polaris-sing-tun-claims",os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC)
    context["procFD"]=os.open("/proc",os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC)
    require(stat.S_IMODE(os.fstat(context["claimsFD"]).st_mode)==0o1777 and os.fstat(context["claimsFD"]).st_uid==0,"actual formal private claims identity differs")


def r_metadata(context,actor,control_fd=None):
    import fcntl
    verifier=context["verifier"];plan=context["plan"];index=context["index"]
    mapping={3:context["netfd"],4:context["userfd"],5:context["mntfd"],6:context["procFD"],7:context["claimsFD"]}
    if index==17:mapping[9]=context["providedTun"]
    if index==0 and actor=="a":mapping[10]=control_fd
    path=Path("/r-metadata")/(actor+".json");writer=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW|os.O_CLOEXEC,0o444)
    reader=None
    try:
        reader=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC);mapping[8]=reader
        binding={key:plan[key] for key in ("profileSha256","batchNonce","sourceCommit","sourceTree","sourceFilesSha256","moduleGraphSha256","elfSha256","configSha256")}
        binding.update(caseNonce=plan["caseNonces"][index],planSha256=context["planSha256"])
        observations={}
        for slot in verifier.role_fds(index,actor):
            fd=mapping[slot];flags=fcntl.fcntl(fd,fcntl.F_GETFL)&os.O_ACCMODE
            require(flags==(os.O_RDWR if slot==9 else os.O_RDONLY),"actual inherited access mode differs")
            value=os.fstat(fd)
            if slot in (3,4,5):
                require(fcntl.ioctl(fd,NS_GET_NSTYPE)=={3:CLONE_NEWNET,4:0x10000000,5:0x00020000}[slot],"actual inherited namespace type differs")
            elif slot in (6,7):require(stat.S_ISDIR(value.st_mode),"actual inherited directory type differs")
            elif slot==8:require(stat.S_ISREG(value.st_mode),"actual metadata regular type differs")
            elif slot==9:require(stat.S_ISCHR(value.st_mode) and os.major(value.st_rdev)==10 and os.minor(value.st_rdev)==200,"actual provided TUN char type differs")
            elif slot==10:require(stat.S_ISFIFO(value.st_mode) and flags==os.O_RDONLY and fcntl.fcntl(fd,fcntl.F_GETFL)&os.O_NONBLOCK,"actual A pipe type/mode differs")
            observations[str(slot)]={**identity(fd),"type":{3:"nsfs",4:"nsfs",5:"nsfs",6:"directory",7:"directory",8:"regular",9:"tun",10:"pipe"}[slot],"access":"readWrite" if slot==9 else "readOnly"}
        metadata={"schema":verifier.METADATA_SCHEMA,"version":1,"profile":R_PROFILE,**binding,"caseID":index,"actor":actor,
                  "fdRoles":{str(fd):verifier.FD_ROLES[fd] for fd in verifier.role_fds(index,actor)},"fdObservedIdentities":observations,
                  "guardianParentIdentities":context["parent"],"caseOptions":plan["cases"][index]["options"][actor],"topology":plan["cases"][index]["topology"],"budgets":verifier.BUDGETS}
        verifier.validate_metadata(metadata);data=verifier.encoded(metadata)
        view=memoryview(data)
        while view:view=view[os.write(writer,view):]
        os.fsync(writer);os.close(writer);writer=None
        require(stat.S_ISREG(os.fstat(reader).st_mode) and os.fstat(reader).st_size==len(data),"actual protected metadata differs")
        # Keep only the read reference. Native inherits no write-capable alias or metadata parent FD.
        context["metadata"].append({"fd":reader,"path":str(path),"identity":identity(reader),"sha256":hashlib.sha256(data).hexdigest()})
        return mapping,metadata
    except BaseException:
        if reader is not None:os.close(reader)
        raise
    finally:
        if writer is not None:os.close(writer)


def r_control_write(fd,child,evidence,deadline):
    # Actual B wait precedes this function; all bytes and EOF are fixed and deadline bounded.
    require(child.returncode==0 and evidence and any(x["phase"]=="priority_refused" for x in evidence),"B actual Wait/typed refusal prerequisite absent")
    payload=b"B_NEW_REFUSED\n";written=0
    import select
    expires=min(deadline,time.monotonic()+1)
    while written<len(payload):
        require(time.monotonic()<expires,"A/B one-shot write budget expired")
        _,ready,_=select.select([], [fd], [], max(0,expires-time.monotonic()))
        if ready:
            try:written+=os.write(fd,payload[written:])
            except BlockingIOError:pass
    os.close(fd)


def r_children(context):
    """One fixed case in this existing worker. Retain every original Popen until actual Wait."""
    import selectors,base64
    verifier=context["verifier"];index=context["index"];deadline=time.monotonic()+15
    selector=selectors.DefaultSelector();children=[];pidfds=[];actors=[];evidence_bytes=0;output_bytes=0;control_read=control_write=None;barrier=None
    def spawn(actor):
        mapping,metadata=r_metadata(context,actor,control_read if actor=="a" else None)
        environment=r_child_environment(os.environ,metadata)
        argv=["-test.v","-test.count=1","-test.run="+verifier.selector(index),"-test.timeout=15s"]
        child=r_spawn("/native.test",argv,mapping,index,actor,environment,children)
        pidfd=os.pidfd_open(child.pid);pidfds.append((child,pidfd))
        entry={"actor":actor,"child":child,"metadata":metadata,"stdout":bytearray(),"stderr":bytearray(),"pendingLines":{"stdout":bytearray(),"stderr":bytearray()},"evidence":[],"wait":None}
        actors.append(entry)
        for stream in ("stdout","stderr"):
            pipe=getattr(child,stream);os.set_blocking(pipe.fileno(),False);selector.register(pipe,selectors.EVENT_READ,(entry,stream))
        record("r-actual-child",caseID=index,actor=actor,pid=child.pid,passFDs=list(sorted(mapping)),metadataSha256=hashlib.sha256(verifier.encoded(metadata)).hexdigest())
        return entry
    try:
        if index==0:
            control_read,control_write=os.pipe2(os.O_CLOEXEC|os.O_NONBLOCK);spawn("a");os.close(control_read);control_read=None
        else:spawn("single")
        while selector.get_map() or any(e["wait"] is None for e in actors):
            require(time.monotonic()<deadline,"R case15s budget expired")
            for key,_ in selector.select(min(0.05,max(0,deadline-time.monotonic()))):
                entry,stream=key.data
                try:chunk=os.read(key.fileobj.fileno(),4096)
                except BlockingIOError:continue
                if not chunk:
                    require(not entry["pendingLines"][stream],"truncated child line")
                    selector.unregister(key.fileobj);key.fileobj.close();continue
                output_bytes+=len(chunk);require(output_bytes<=verifier.MAX_STDOUT,"combined A/B output budget exceeded")
                entry[stream].extend(chunk);pending=entry["pendingLines"][stream];pending.extend(chunk)
                require(len(pending)<=verifier.MAX_EVIDENCE,"unbounded/no-newline child output")
                while b"\n" in pending:
                    line,rest=pending.split(b"\n",1);pending[:]=rest
                    if stream=="stdout" and line.startswith(b"R_NATIVE_EVIDENCE "):
                        raw=line[len(b"R_NATIVE_EVIDENCE "):];evidence_bytes+=len(raw)
                        require(evidence_bytes<=verifier.MAX_EVIDENCE,"case cumulative evidence budget exceeded")
                        event=verifier.closed_json(raw,verifier.MAX_EVIDENCE);entry["evidence"].append(event)
                        require(event.get("caseID")==index and event.get("actor")==entry["actor"] and event.get("caseNonce")==entry["metadata"]["caseNonce"] and event.get("batchNonce")==entry["metadata"]["batchNonce"],"actual child evidence association differs")
            for entry in list(actors):
                child=entry["child"]
                if entry["wait"] is None and child.poll() is not None:
                    entry["wait"]={"pid":child.pid,"exitStatus":child.wait(timeout=max(0.001,min(1,deadline-time.monotonic()))),"actual":True,"timedOut":False}
                    require(entry["wait"]["exitStatus"]==0,"native child failed; no retry/next case")
            if index==0 and len(actors)==1:
                a=actors[0];ready=any(x.get("phase")=="running" and x.get("facts",{}).get("phaseLabel")=="independent_a_running_before_b" for x in a["evidence"])
                if ready:
                    require(a["child"].poll() is None,"A died before B birth");spawn("b")
                elif a["wait"] is not None:raise RuntimeError("A exited before actual ready barrier")
            if index==0 and len(actors)==2 and actors[1]["wait"] is not None and barrier is None and not any(key.data[0] is actors[1] for key in selector.get_map().values()):
                a,b=actors;require(a["child"].poll() is None,"A is not alive at B refusal")
                # Full independent typed refusal closure is checked before the one write.
                binding={key:b["metadata"][key] for key in ("profileSha256","batchNonce","caseNonce","sourceCommit","sourceTree","sourceFilesSha256","moduleGraphSha256","elfSha256","configSha256","planSha256")}
                producer=next(x for x in context["plan"]["selectedTopLists"] if x["top"]==verifier.CASES[index][0])
                actor_receipt={"actor":"b","metadata":b["metadata"],"pid":b["child"].pid,"wait":b["wait"],"stdout":base64.b64encode(b["stdout"]).decode(),"stderr":base64.b64encode(b["stderr"]).decode(),"list":producer,"evidence":b["evidence"]}
                verifier.verify_actor(actor_receipt,index,"b",binding)
                refusal=next((x for x in b["evidence"] if x["phase"]=="priority_refused"),None)
                require(refusal is not None and refusal["facts"]=={"newReturnedNil":True,"errno":"EEXIST","source":"priority_preflight","tunOpenCount":0,"startCount":0,"callbackCount":0} and a["evidence"][0]["birth"]!=b["evidence"][0]["birth"],"actual distinct B preflight refusal absent")
                r_control_write(control_write,b["child"],b["evidence"],deadline);control_write=None
                barrier={"bytes":base64.b64encode(verifier.BARRIER).decode(),"aReadyBeforeBStart":True,"bWaitBeforeWrite":True,"closedAfterWrite":True,"aAliveAtWrite":True}
        result=[]
        for entry in actors:
            producer=next(x for x in context["plan"]["selectedTopLists"] if x["top"]==verifier.CASES[index][0])
            result.append({"actor":entry["actor"],"metadata":entry["metadata"],"pid":entry["child"].pid,"wait":entry["wait"],"stdout":base64.b64encode(entry["stdout"]).decode(),"stderr":base64.b64encode(entry["stderr"]).decode(),"list":producer,"evidence":entry["evidence"]})
        return result,barrier
    finally:
        # Every actual Popen is in the original worker list immediately at birth, even restore error.
        wait_deadline=time.monotonic()+5;context["cleanupDeadline"]=wait_deadline;errors=[]
        for child in children:
            try:
                if child.poll() is None:
                    try:os.killpg(child.pid,signal.SIGKILL)
                    except ProcessLookupError:pass
                    for owner,pidfd in pidfds:
                        if owner is child:
                            try:signal.pidfd_send_signal(pidfd,signal.SIGKILL)
                            except ProcessLookupError:pass
                child.wait(timeout=max(0.001,wait_deadline-time.monotonic()))
            except BaseException as error:errors.append(type(error).__name__)
        for _,pidfd in pidfds:os.close(pidfd)
        for fd in (control_read,control_write):
            if fd is not None:os.close(fd)
        for key in list(selector.get_map().values()):key.fileobj.close()
        selector.close()
        context["childrenSettled"]=not errors
        require(not errors,"R actual child Wait unsettled: "+",".join(errors))


def r_cleanup(context):
    errors=[];index=context["index"];deadline=context["cleanupDeadline"]
    def attempt(label,operation):
        try:
            require(time.monotonic()<deadline,"R cleanup5s budget expired")
            operation()
        except BaseException as error:errors.append(label+":"+type(error).__name__)
    def restoration():
        if index==16:
            actual=Path("/proc/sys/net/ipv4/conf/rnt16/rp_filter")
            require(actual.stat().st_ino==context["writerBefore"]["identity"] and actual.read_bytes()==b"1\n","actual R rp_filter restoration failed")
            context["writerAfter"]={"identity":actual.stat().st_ino,"value":actual.read_bytes().decode()}
        persistent=context["plan"]["cases"][index]["topology"]["persistentBefore"]
        if persistent is not None:
            context["persistentAfterObserved"]=r_link_observation(persistent["name"])
            require(r_link_configuration(context["persistentBeforeObserved"])==r_link_configuration(context["persistentAfterObserved"]),"actual persistent MTU/up/address restoration differs")
        if index==17:require(identity(context["providedTun"])==context["providedTunIdentity"],"G original provided FD identity changed")
    attempt("subjectRestoration",restoration)
    for item in context.get("setupObjects",[]):
        def dispose(item=item):
            actual=r_link_observation(item["name"])
            require(actual["link"]["ifindex"]==item["ifindex"],"G-created fixture link identity changed")
            context.setdefault("fixtureBeforeDisposal",[]).append(actual)
            r_ip("link","delete","dev",item["name"])
        attempt("dispose:"+item["name"],dispose)
    for path,item in context.get("sysctls",{}).items():
        def restore(path=path,item=item):
            require(Path(path).stat().st_ino==item["identity"],"G setup sysctl leaf identity changed")
            Path(path).write_bytes(item["bytes"]);require(Path(path).read_bytes()==item["bytes"],"G private setup value restore failed")
        attempt("restore:"+path,restore)
    for item in context["metadata"]:
        def validate(item=item):
            require(identity(item["fd"])==item["identity"] and hashlib.sha256(Path(item["path"]).read_bytes()).hexdigest()==item["sha256"],"original protected metadata changed")
        attempt("metadata",validate)
        try:os.close(item["fd"])
        except OSError as error:errors.append("metadataClose:"+type(error).__name__)
    for key in ("providedTun","claimsFD","procFD"):
        if context.get(key) is not None:
            try:os.close(context[key])
            except OSError as error:errors.append(key+":"+type(error).__name__)
    require(not errors,"R fixture restoration/disposal Unknown: "+",".join(errors))
    return {"restored":True,"fixtureDisposal":"ActualRestoredAndDisposed",
            "persistentBefore":r_link_configuration(context["persistentBeforeObserved"]) if "persistentBeforeObserved" in context else None,
            "persistentAfter":r_link_configuration(context["persistentAfterObserved"]) if "persistentAfterObserved" in context else None,
            "writerBefore":context.get("writerBefore"),"writerAfter":context.get("writerAfter"),
            "providedOriginalIdentity":context.get("providedTunIdentity"),
            "disposedObjects":[{"name":x["link"]["ifname"],"ifindex":x["link"]["ifindex"]} for x in context.get("fixtureBeforeDisposal",[])],
            "protectedMetadataUnchanged":True,"setupLeavesRestored":True}


def r_worker_body(args,netfd,mntfd,additional_fds,parent,current):
    r_execution_gate(os.environ)
    verifier=r_module();plan_bytes=verifier.bounded_file(args.r_plan,4194304)
    require(hashlib.sha256(plan_bytes).hexdigest()==args.r_plan_sha256,"actual worker protected plan differs")
    plan=verifier.closed_json(plan_bytes,4194304);r_plan(plan,verifier)
    context={"plan":plan,"verifier":verifier,"index":args.r_case,"netfd":netfd,"mntfd":mntfd,"userfd":additional_fds["user"],"parent":parent,"current":current,"metadata":[],"planSha256":args.r_plan_sha256}
    result=None;started=time.monotonic()
    try:
        # The original private root is reused. R adds only one dev node and formal private claims.
        fence=private_root(args.evidence,args.binary,netfd,False,context)
        require(time.monotonic()-started<=10,"R private setup10s budget exceeded")
        record("r-filesystem-profile",**fence)
        actors,barrier=r_children(context)
        binding={key:actors[0]["metadata"][key] for key in ("profileSha256","batchNonce","caseNonce","sourceCommit","sourceTree","sourceFilesSha256","moduleGraphSha256","elfSha256","configSha256","planSha256")}
        planned=plan["cases"][args.r_case]
        producer=next(x for x in plan["selectedTopLists"] if x["top"]==verifier.CASES[args.r_case][0])
        for actor in actors:verifier.verify_actor(actor,args.r_case,actor["actor"],binding,planned,producer)
        result={"schema":verifier.ENVELOPE_SCHEMA,"caseID":args.r_case,"selected":"/".join(verifier.CASES[args.r_case]),"binding":binding,"actors":actors,"barrier":barrier,"evidenceClass":"ActualNative"}
        return 0
    finally:
        cleanup_started=time.monotonic();context.setdefault("cleanupDeadline",cleanup_started+5)
        try:
            waited,settled=reap_all(context["cleanupDeadline"])
            require(settled and context.get("childrenSettled",True),"R actual Wait unsettled; fixture disposition remains Unknown")
            cleanup=r_cleanup(context)
            require(time.monotonic()<context["cleanupDeadline"],"R original worker actual Wait/cleanup unsettled")
            if result is not None:
                result["fixture"]=cleanup
                record("r-case-result",envelope=result,fixture=cleanup,actualWait=waited,allChildrenWaited=settled,cleanupWithinMillis=int((time.monotonic()-cleanup_started)*1000))
        except BaseException as error:
            record("r-case-unknown",caseID=args.r_case,error=type(error).__name__,subjectCleanup="Unknown",fixtureDisposal="Unknown")
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=(N_PROFILE, R_PROFILE), default=N_PROFILE)
    parser.add_argument("--r-plan")
    parser.add_argument("--r-plan-sha256")
    parser.add_argument("--r-profile-json")
    parser.add_argument("--r-config-json")
    parser.add_argument("--r-case", type=int, choices=range(28))
    parser.add_argument("--guardian", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--frozen-guardian", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--cancel-probe", choices=("term", "kill"))
    parser.add_argument("--probe", choices=("clean", "timeout"))
    parser.add_argument("--binary")
    parser.add_argument("--sha256")
    parser.add_argument("--expected-test", action="append", default=[])
    parser.add_argument("--deadline", type=int, default=45)
    parser.add_argument("--parent-net", help=argparse.SUPPRESS)
    parser.add_argument("--parent-mnt", help=argparse.SUPPRESS)
    parser.add_argument("--parent-identities", help=argparse.SUPPRESS)
    parser.add_argument("--evidence", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.profile == R_PROFILE:
        # Before any namespace, metadata/native FD, root bootstrap or source copy.
        r_execution_gate(os.environ)
        require(not args.probe and not args.cancel_probe and all((args.r_plan, args.r_plan_sha256, args.r_profile_json, args.r_config_json, args.r_case is not None)), "R explicit frozen plan/profile/config/case required")
        if "--deadline" not in sys.argv:
            args.deadline = 30
    else:
        require(not any((args.r_plan, args.r_plan_sha256, args.r_profile_json, args.r_config_json, args.r_case is not None)), "R inputs require explicit R-only profile")
    if args.cancel_probe:
        require(not args.guardian and not args.worker and not args.binary, "cancel probe is standalone")
        return cancellation_probe(args.cancel_probe)
    require(args.guardian != args.worker, "select --guardian")
    if args.guardian and not args.frozen_guardian:
        require(os.geteuid() == 0, "guardian bootstrap requires root")
        launch_directory = secure_directory("polaris-native-launch-")
        frozen_script, frozen_hash = freeze_worker(launch_directory)
        if args.profile == R_PROFILE:
            r_inputs(args, launch_directory)
        # Re-execute both guardian and worker from the same protected bytes.
        os.execve("/usr/bin/python3", ["/usr/bin/python3", "-I", str(frozen_script),
                  *sys.argv[1:], "--frozen-guardian"],
                  {"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "PYTHONDONTWRITEBYTECODE": "1", **({"POLARIS_R_NATIVE_RUN": os.environ.get("POLARIS_R_NATIVE_RUN", ""), "POLARIS_NO_KERNEL_RUN": os.environ.get("POLARIS_NO_KERNEL_RUN", "")} if args.profile == R_PROFILE else {})})
    if args.guardian:
        value, parent = SCRIPT.lstat(), SCRIPT.parent.lstat()
        require(value.st_uid == 0 and parent.st_uid == 0 and not value.st_mode & 0o022
                and not parent.st_mode & 0o022 and not SCRIPT.is_symlink(), "guardian source is not root protected")
    return worker(args) if args.worker else guardian(args)


def cancellation_probe(kind):
    """A still-live outer root owner waits guardian and its adopted descendants."""
    require(os.geteuid() == 0, "cancellation probe requires root")
    require(ctypes.CDLL(None, use_errno=True).prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) == 0, "outer subreaper failed")
    directory = secure_directory("polaris-native-cancellation-")
    frozen, script_hash = freeze_worker(directory)
    receipt = {"schema": "polaris-native-cancellation-v1", "harnessSha256": script_hash,
               "kind": kind, "result": "Unknown"}
    child, pidfd = None, None
    try:
        with (directory / "guardian.ndjson").open("x") as output:
            child = subprocess.Popen(["/usr/bin/python3", "-I", str(frozen), "--guardian", "--frozen-guardian", "--probe", "timeout", "--deadline", "15"],
                                     stdout=output, stderr=output, stdin=subprocess.DEVNULL,
                                     start_new_session=True, close_fds=True,
                                     env={"PATH": "/usr/sbin:/usr/bin:/sbin:/bin"})
            pidfd = os.pidfd_open(child.pid)
            expires = time.monotonic()+8
            inner, namespace, ready = None, None, False
            while child.poll() is None and time.monotonic() < expires:
                for line in (directory / "guardian.ndjson").read_text().splitlines():
                    try:
                        event = json.loads(line)
                        if event.get("kind") == "guardian-evidence":
                            inner = Path(event["directory"])
                    except ValueError:
                        continue
                if inner and (inner / "worker.ndjson").exists():
                    for line in (inner / "worker.ndjson").read_text().splitlines():
                        try:
                            event = json.loads(line)
                            if event.get("kind") == "namespace":
                                namespace = event["net"]
                            if event.get("kind") == "setsid-descendant":
                                ready = True
                        except ValueError:
                            continue
                if ready:
                    break
                time.sleep(0.01)
            require(ready and namespace and child.poll() is None, "cancellation probe not ready")
            signal.pidfd_send_signal(pidfd, signal.SIGTERM if kind == "term" else signal.SIGKILL)
            receipt["guardianExit"] = child.wait(timeout=5)
            receipt["reaped"], receipt["allChildrenWaited"] = reap_all(time.monotonic()+5)
            receipt["namespaceReferences"] = namespace_references(namespace)
            receipt["innerEvidence"] = str(inner)
            if kind == "term":
                inner_receipt = json.loads((inner / "receipt.json").read_text())
                receipt["innerResult"] = inner_receipt["result"]
                expected = inner_receipt["result"] == "Unknown" and inner_receipt.get("allChildrenWaited") is True
            else:
                expected = receipt["guardianExit"] == -signal.SIGKILL
            refs = receipt["namespaceReferences"]
            if expected and receipt["allChildrenWaited"] and not refs["references"] and not refs["errors"]:
                receipt["result"] = "ProbePass"
    except Exception as error:
        receipt["error"] = str(error)
        if child is not None and child.poll() is None and pidfd is not None:
            signal.pidfd_send_signal(pidfd, signal.SIGKILL)
            child.wait(timeout=5)
            receipt["reaped"], receipt["allChildrenWaited"] = reap_all(time.monotonic()+5)
    finally:
        if pidfd is not None:
            os.close(pidfd)
        (directory / "receipt.json").write_text(json.dumps(receipt, indent=2, sort_keys=True)+"\n")
        for path in directory.iterdir():
            if path.is_file():
                path.chmod(0o644)
        directory.chmod(0o755)
    print(json.dumps({"receipt": str(directory / "receipt.json"), "result": receipt["result"]}), flush=True)
    return 0 if receipt["result"] == "ProbePass" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"native isolation failed: {error}", file=sys.stderr)
        sys.exit(1)
