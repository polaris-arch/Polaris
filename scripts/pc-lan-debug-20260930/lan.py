#!/usr/bin/env python3
"""Finite PC echo source. Offline verification never authenticates device evidence."""
import argparse
import base64
import ctypes
import errno
import hashlib
import ipaddress
import json
import os
import secrets
import select
import signal
import socket
import stat
import subprocess
import sys
import threading
import time
from pathlib import Path

SCOPE_SHA = "7d08501fbfb7d6269cf681d1de3f9d5e0138f3b89e4cd038de8de5ab372661c2"
MAX_JSON = 65536
MAX_SOURCE = 262144
MAX_RECORD = 16384
MAX_EVENTS = 128
MAX_OUTPUT = 131072
MAX_REQUESTS = 32
CASE_IDS = tuple(p + "-" + k for p in ("tcp", "udp") for k in
                 ("wrong-nonce", "foreign-peer", "wrong-port-admission"))
BINDING_KEYS = {"runId", "pcCandidateSha", "androidCandidateSha", "receiverSourceSha256",
                "androidPackageSha256", "destinationIPv4", "expectedSenderIPv4", "tcpPort",
                "udpPort", "nonceSha256", "lifetimeSeconds", "maxPayloadBytes",
                "maxConnectionsOrDatagrams"}
COUNTER_KEYS = {"total", "pending", "matched", "wrongNonce", "foreignPeer", "readFailed", "malformed",
                "echoWritten", "writeFailed"}
SOURCE_ITEMS = (("controller-receiver", "lan.py"), ("windows-private-file", "private-windows.ps1"))


class Fault(Exception):
    def __init__(self, code):
        self.code = code
        super().__init__(code)


def require(condition, code="InvalidShape"):
    if not condition:
        raise Fault(code)


def shape(value, keys):
    require(type(value) is dict and set(value) == set(keys))
    return value


def choice(value, choices):
    require(type(value) is str and value in choices, "InvalidDiscriminator")
    return value


def raw_base64(value, maximum=MAX_RECORD):
    require(type(value) is str and value.isascii() and 0 < len(value) <= 4 * ((maximum + 2) // 3),
            "InvalidBase64")
    try:
        raw = base64.b64decode(value, validate=True)
    except ValueError:
        raise Fault("InvalidBase64") from None
    require(0 < len(raw) <= maximum and base64.b64encode(raw).decode("ascii") == value, "InvalidBase64")
    return raw


def integer(value, low=0, high=(1 << 63) - 1):
    require(type(value) is int and low <= value <= high)
    return value


def hex_value(value, low, high=None):
    require(type(value) is str and low <= len(value) <= (low if high is None else high)
            and all(c in "0123456789abcdef" for c in value))
    return value


def ipv4(value):
    require(type(value) is str and len(value) <= 15)
    try:
        address = ipaddress.IPv4Address(value)
    except ipaddress.AddressValueError:
        raise Fault("InvalidIPv4") from None
    require(str(address) == value and any(address in ipaddress.IPv4Network(network)
            for network in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")), "InvalidIPv4")
    require(int(value.split(".")[-1]) in range(1, 255), "InvalidIPv4")
    return value


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True,
                      allow_nan=False).encode("utf-8")


def sha(data):
    return hashlib.sha256(data).hexdigest()


def digest(domain, data):
    return sha(domain.encode("ascii") + b"\0" + data)


def decode(data, maximum=MAX_JSON):
    require(type(data) is bytes and 0 < len(data) <= maximum, "InputSize")

    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, "DuplicateKey")
            result[key] = value
        return result

    def no_float(_):
        raise Fault("NonIntegerNumber")

    try:
        value = json.loads(data.decode("utf-8", errors="strict"), object_pairs_hook=pairs,
                           parse_float=no_float, parse_constant=no_float)
    except (ValueError, UnicodeError, RecursionError):
        raise Fault("InvalidJSON") from None

    def bounded(item, depth=0):
        require(depth <= 16, "NestingLimit")
        if type(item) is str:
            require(len(item) <= 32768, "StringLimit")
        elif type(item) is int:
            integer(item, -(1 << 31))
        elif type(item) is list:
            require(len(item) <= MAX_EVENTS, "ArrayLimit")
            for child in item:
                bounded(child, depth + 1)
        elif type(item) is dict:
            require(len(item) <= 40, "ObjectLimit")
            for key, child in item.items():
                bounded(key, depth + 1)
                bounded(child, depth + 1)
        else:
            require(item is None or type(item) is bool)
    bounded(value)
    return value


def binding(value):
    shape(value, BINDING_KEYS)
    hex_value(value["runId"], 32, 64)
    for key in ("pcCandidateSha", "androidCandidateSha"):
        hex_value(value[key], 40)
    for key in ("receiverSourceSha256", "androidPackageSha256", "nonceSha256"):
        hex_value(value[key], 64)
    ipv4(value["destinationIPv4"])
    ipv4(value["expectedSenderIPv4"])
    for key in ("tcpPort", "udpPort"):
        integer(value[key], 49152, 65535)
    require(value["tcpPort"] != value["udpPort"], "DuplicatePort")
    integer(value["lifetimeSeconds"], 1, 120)
    require(integer(value["maxPayloadBytes"]) == 65)
    require(integer(value["maxConnectionsOrDatagrams"]) == MAX_REQUESTS)
    return value


def plan(value):
    shape(value, {"schema", "scopeSha256", "binding", "ownerInput", "echoNonce", "sourceKind"})
    require(value["schema"] == "polaris-pc-lan-plan-v1" and value["scopeSha256"] == SCOPE_SHA)
    binding(value["binding"])
    shape(value["ownerInput"], {"kind", "destinationIPv4"})
    require(value["sourceKind"] in ("Synthetic", "OperatorDeclared"))
    require(value["ownerInput"]["kind"] == value["sourceKind"] and
            value["ownerInput"]["destinationIPv4"] == value["binding"]["destinationIPv4"])
    hex_value(value["echoNonce"], 32, 64)
    require(sha(value["echoNonce"].encode("ascii")) == value["binding"]["nonceSha256"], "NonceBinding")
    return value


def plan_sha(raw):
    value = plan(decode(raw))
    require(canonical(value) == raw, "NonCanonicalPlan")
    return sha(raw)


def source_manifest(items):
    require(set(items) == {path for _, path in SOURCE_ITEMS}, "SourceInventory")
    require(all(type(raw) is bytes and 0 < len(raw) <= MAX_SOURCE for raw in items.values()), "SourceSize")
    manifest = {"schema": "polaris-pc-lan-execution-source-v1", "items": [
        {"role": role, "path": path, "sha256": sha(items[path])} for role, path in SOURCE_ITEMS]}
    raw = canonical(manifest)
    return manifest, digest("polaris-pc-lan-execution-source-v1", raw)


def claims(verdict, reasons=()):
    return {"schema": "polaris-pc-lan-result-v1", "verdict": verdict, "reasons": list(reasons),
            "claims": [], "globalNoOwner": False, "managedReady": False,
            "releaseReady": False, "networkExact": False, "outboundReady": False}


def tuple_value(value):
    shape(value, {"address", "port", "protocol"})
    ipv4(value["address"])
    integer(value["port"], 49152, 65535)
    require(value["protocol"] in ("tcp", "udp"))
    return value


def bytes_union(value, absent):
    require(type(value) is dict and "kind" in value)
    choice(value["kind"], (absent, "Bytes"))
    if value["kind"] == absent:
        shape(value, {"kind"})
    else:
        shape(value, {"kind", "count", "sha256"})
        require(value["kind"] == "Bytes")
        integer(value["count"], 0, 1500)
        hex_value(value["sha256"], 64)


def native_union(value):
    require(type(value) is dict and "kind" in value)
    choice(value["kind"], ("Unknown", "Reference"))
    if value["kind"] == "Unknown":
        shape(value, {"kind"})
    else:
        shape(value, {"kind", "sha256"})
        require(value["kind"] == "Reference")
        hex_value(value["sha256"], 64)


def public_witness(value):
    shape(value, {"schema", "pcRunId", "pcPlanSha256", "readyReceiptSha256", "receiverInstanceId",
                  "socketInstanceId", "protocol", "caseId", "caseKind", "attemptSeq", "requestedTuple",
                  "approvedTuple", "sent", "returned", "outcome", "rootNativeWitness"})
    require(value["schema"] == "polaris-pc-echo-public-witness-v1")
    hex_value(value["pcRunId"], 32, 64)
    for key in ("pcPlanSha256", "readyReceiptSha256"):
        hex_value(value[key], 64)
    for key in ("receiverInstanceId", "socketInstanceId"):
        hex_value(value[key], 32)
    require(value["protocol"] in ("tcp", "udp") and value["caseId"] in CASE_IDS)
    require(value["caseKind"] in ("Positive", "WrongNonce", "ForeignPeer", "WrongPortAdmission"))
    integer(value["attemptSeq"], 1, 32)
    tuple_value(value["requestedTuple"])
    tuple_value(value["approvedTuple"])
    bytes_union(value["sent"], "NotSent")
    bytes_union(value["returned"], "NotReceived")
    require(value["outcome"] in ("NotObserved", "RejectedBeforeOutbound", "ExactEcho", "NoEcho",
            "TransportFailure", "Canceled", "StaleScope", "Unsupported"))
    native_union(value["rootNativeWitness"])
    return value


def target(value):
    shape(value, {"schema", "pcRunId", "pcPlanSha256", "readyReceiptSha256", "pcCandidateSha",
                  "androidCandidateSha", "receiverSourceSha256", "androidPackageSha256", "receiverInstanceId",
                  "tcpSocketInstanceId", "udpSocketInstanceId", "destinationIPv4", "expectedSenderIPv4",
                  "tcpPort", "udpPort", "echoNonce", "echoNonceSha256", "lifetimeSeconds", "maxRequests"})
    require(value["schema"] == "polaris-pc-echo-target-v1")
    binding({"runId": value["pcRunId"], **{k: value[k] for k in BINDING_KEYS - {
        "runId", "nonceSha256", "maxPayloadBytes", "maxConnectionsOrDatagrams"}},
        "nonceSha256": value["echoNonceSha256"], "maxPayloadBytes": 65,
        "maxConnectionsOrDatagrams": value["maxRequests"]})
    for key in ("pcPlanSha256", "readyReceiptSha256"):
        hex_value(value[key], 64)
    for key in ("receiverInstanceId", "tcpSocketInstanceId", "udpSocketInstanceId"):
        hex_value(value[key], 32)
    require(value["tcpSocketInstanceId"] != value["udpSocketInstanceId"])
    hex_value(value["echoNonce"], 32, 64)
    require(sha(value["echoNonce"].encode("ascii")) == value["echoNonceSha256"], "NonceBinding")
    return value


def target_from_original_ready(raw_plan, raw_ready):
    """Private codec bridge only; the root driver must independently check current readiness."""
    p = plan(decode(raw_plan))
    record = receiver_record(raw_ready)
    require(record["phase"] == "Ready" and record["binding"] == p["binding"] and
            record["pcPlanSha256"] == plan_sha(raw_plan), "ReadyBinding")
    sockets = {fact["protocol"]: fact for fact in record["sockets"]}
    b = p["binding"]
    value = {"schema": "polaris-pc-echo-target-v1", "pcRunId": b["runId"],
        "pcPlanSha256": plan_sha(raw_plan), "readyReceiptSha256": sha(raw_ready),
        "receiverInstanceId": record["receiverInstanceId"],
        "tcpSocketInstanceId": sockets["tcp"]["socketInstanceId"],
        "udpSocketInstanceId": sockets["udp"]["socketInstanceId"],
        "echoNonce": p["echoNonce"], "echoNonceSha256": b["nonceSha256"],
        "maxRequests": b["maxConnectionsOrDatagrams"], **{key: b[key] for key in (
            "pcCandidateSha", "androidCandidateSha", "receiverSourceSha256", "androidPackageSha256",
            "destinationIPv4", "expectedSenderIPv4", "tcpPort", "udpPort", "lifetimeSeconds")}}
    return target(value)


def _private_acl(fd):
    if sys.platform == "darwin":
        lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        lib.acl_get_fd_np.argtypes = [ctypes.c_int, ctypes.c_int]
        lib.acl_get_fd_np.restype = ctypes.c_void_p
        lib.acl_get_entry.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.POINTER(ctypes.c_void_p)]
        lib.acl_free.argtypes = [ctypes.c_void_p]
        acl = lib.acl_get_fd_np(fd, 0x100)
        require(bool(acl), "ACLUnverified")
        try:
            entry = ctypes.c_void_p()
            require(lib.acl_get_entry(acl, 0, ctypes.byref(entry)) == 0, "ExtendedACL")
        finally:
            lib.acl_free(acl)
    else:
        try:
            os.getxattr(fd, "system.posix_acl_access")
        except OSError as error:
            require(error.errno in (errno.ENODATA, errno.ENOTSUP, errno.EOPNOTSUPP), "ACLUnverified")
        else:
            raise Fault("ExtendedACL")


def _posix_parent(path, private=False):
    p = Path(path)
    require(p.is_absolute() and ".." not in p.parts, "UnsafePath")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK
    fd = os.open("/", flags)
    try:
        for part in p.parts[1:-1]:
            next_fd = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = next_fd
            info = os.fstat(fd)
            require(stat.S_ISDIR(info.st_mode) and info.st_uid in (0, os.geteuid()), "UnsafeParent")
            require(info.st_mode & 0o022 == 0 or (info.st_uid == 0 and info.st_mode & stat.S_ISVTX), "WritableParent")
        info = os.fstat(fd)
        if private:
            require(info.st_uid == os.geteuid() and info.st_mode & 0o077 == 0, "PrivateParentRequired")
            _private_acl(fd)
        return fd, p.name
    except BaseException:
        os.close(fd)
        raise


def read_file(path, maximum=MAX_JSON, private=True):
    if os.name == "nt":
        return WinFiles().read(path, maximum, private)
    parent, name = _posix_parent(path, private)
    fd = None
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
        before = os.fstat(fd)
        require(stat.S_ISREG(before.st_mode) and before.st_size <= maximum, "RegularBoundedFileRequired")
        if private:
            require(before.st_uid == os.geteuid() and before.st_mode & 0o077 == 0, "PrivateFileRequired")
            _private_acl(fd)
        chunks = bytearray()
        while len(chunks) <= maximum:
            block = os.read(fd, min(65536, maximum + 1 - len(chunks)))
            if not block:
                break
            chunks.extend(block)
        after = os.fstat(fd)
        stable = lambda s: (s.st_dev, s.st_ino, s.st_mode, s.st_uid, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
        require(len(chunks) <= maximum and stable(before) == stable(after)
                and len(chunks) == after.st_size, "FileChanged")
        return bytes(chunks)
    finally:
        if fd is not None:
            os.close(fd)
        os.close(parent)


def create_file(path, data):
    require(type(data) is bytes and len(data) <= 4 * MAX_OUTPUT, "OutputSize")
    if os.name == "nt":
        return WinFiles().create(path, data)
    parent, name = _posix_parent(path, True)
    fd = None
    try:
        fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                     0o600, dir_fd=parent)
        info = os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid() and info.st_mode & 0o077 == 0)
        _private_acl(fd)
        remaining = memoryview(data)
        while remaining:
            count = os.write(fd, remaining)
            require(count > 0, "FileWriteUnknown")
            remaining = remaining[count:]
        os.fsync(fd)
    finally:
        if fd is not None:
            os.close(fd)
        os.close(parent)


def create_directory(path):
    if os.name == "nt":
        return WinFiles().directory(path)
    parent, name = _posix_parent(path)
    try:
        os.mkdir(name, 0o700, dir_fd=parent)
        fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
        try:
            info = os.fstat(fd)
            require(info.st_uid == os.geteuid() and info.st_mode & 0o077 == 0, "PrivateDirectoryRequired")
            _private_acl(fd)
        finally:
            os.close(fd)
    finally:
        os.close(parent)


class WinFiles:
    """Existing kernel32/advapi32 only; original handles, protected owner-only creation."""
    def __init__(self):
        require(os.name == "nt", "WindowsNativeUnverified")
        from ctypes import wintypes as w
        self.w = w
        self.k = ctypes.WinDLL("kernel32", use_last_error=True)
        self.a = ctypes.WinDLL("advapi32", use_last_error=True)
        signatures = [
            (self.k, "CreateFileW", w.HANDLE, [w.LPCWSTR, w.DWORD, w.DWORD, w.LPVOID, w.DWORD, w.DWORD, w.HANDLE]),
            (self.k, "CreateDirectoryW", w.BOOL, [w.LPCWSTR, w.LPVOID]),
            (self.k, "CloseHandle", w.BOOL, [w.HANDLE]),
            (self.k, "GetCurrentProcess", w.HANDLE, []),
            (self.k, "GetFileType", w.DWORD, [w.HANDLE]),
            (self.k, "GetFileInformationByHandle", w.BOOL, [w.HANDLE, w.LPVOID]),
            (self.k, "ReadFile", w.BOOL, [w.HANDLE, w.LPVOID, w.DWORD, w.LPVOID, w.LPVOID]),
            (self.k, "WriteFile", w.BOOL, [w.HANDLE, w.LPVOID, w.DWORD, w.LPVOID, w.LPVOID]),
            (self.k, "FlushFileBuffers", w.BOOL, [w.HANDLE]),
            (self.k, "LocalFree", w.HANDLE, [w.HANDLE]),
            (self.a, "OpenProcessToken", w.BOOL, [w.HANDLE, w.DWORD, w.LPVOID]),
            (self.a, "GetTokenInformation", w.BOOL, [w.HANDLE, ctypes.c_int, w.LPVOID, w.DWORD, w.LPVOID]),
            (self.a, "ConvertSidToStringSidW", w.BOOL, [w.LPVOID, w.LPVOID]),
            (self.a, "ConvertStringSecurityDescriptorToSecurityDescriptorW", w.BOOL, [w.LPCWSTR, w.DWORD, w.LPVOID, w.LPVOID]),
            (self.a, "GetSecurityInfo", w.DWORD, [w.HANDLE, ctypes.c_int, w.DWORD, w.LPVOID, w.LPVOID, w.LPVOID, w.LPVOID, w.LPVOID]),
            (self.a, "GetSecurityDescriptorControl", w.BOOL, [w.LPVOID, w.LPVOID, w.LPVOID]),
            (self.a, "GetAce", w.BOOL, [w.LPVOID, w.DWORD, w.LPVOID]),
            (self.a, "EqualSid", w.BOOL, [w.LPVOID, w.LPVOID]),
        ]
        for library, name, result, args in signatures:
            function = getattr(library, name)
            function.restype, function.argtypes = result, args
        token = w.HANDLE()
        require(self.a.OpenProcessToken(self.k.GetCurrentProcess(), 8, ctypes.byref(token)), "WindowsToken")
        try:
            needed = w.DWORD()
            self.a.GetTokenInformation(token, 1, None, 0, ctypes.byref(needed))
            require(0 < needed.value <= 65536, "WindowsToken")
            self.token = ctypes.create_string_buffer(needed.value)
            require(self.a.GetTokenInformation(token, 1, self.token, needed.value, ctypes.byref(needed)), "WindowsToken")
            self.sid = ctypes.c_void_p.from_buffer(self.token).value
            sid_text = ctypes.c_void_p()
            require(self.a.ConvertSidToStringSidW(self.sid, ctypes.byref(sid_text)), "WindowsToken")
            try:
                self.sid_text = ctypes.wstring_at(sid_text.value)
            finally:
                self.k.LocalFree(sid_text)
        finally:
            self.close_all([token])

    def close_all(self, handles):
        failed = []
        for handle in handles:
            if not self.k.CloseHandle(handle):
                failed.append(handle)
        if failed:
            UNRESOLVED_FILE_HANDLES.extend((self, handle) for handle in failed)
            raise Fault("WindowsHandleCloseUnknown")

    def metadata(self, handle, directory=False):
        w = self.w
        class Info(ctypes.Structure):
            _fields_ = [("attributes", w.DWORD), ("created", w.FILETIME), ("accessed", w.FILETIME),
                        ("written", w.FILETIME), ("volume", w.DWORD), ("sizeHigh", w.DWORD),
                        ("sizeLow", w.DWORD), ("links", w.DWORD), ("indexHigh", w.DWORD), ("indexLow", w.DWORD)]
        info = Info()
        require(self.k.GetFileType(handle) == 1 and self.k.GetFileInformationByHandle(handle, ctypes.byref(info)), "WindowsRegularHandle")
        require(not info.attributes & 0x400 and bool(info.attributes & 0x10) == directory, "WindowsReparseOrType")
        return (info.volume, info.indexHigh, info.indexLow, info.attributes,
                (info.sizeHigh << 32) | info.sizeLow, info.written.dwHighDateTime, info.written.dwLowDateTime)

    def security(self, handle):
        owner, dacl, descriptor = ctypes.c_void_p(), ctypes.c_void_p(), ctypes.c_void_p()
        require(self.a.GetSecurityInfo(handle, 1, 5, ctypes.byref(owner), None, ctypes.byref(dacl), None,
                                       ctypes.byref(descriptor)) == 0, "WindowsSecurityUnknown")
        try:
            control = ctypes.c_ushort()
            revision = self.w.DWORD()
            require(self.a.GetSecurityDescriptorControl(descriptor, ctypes.byref(control), ctypes.byref(revision)), "WindowsSecurityUnknown")
            require(control.value & 0x1000 and dacl.value and self.a.EqualSid(owner, self.sid), "WindowsPrivateDACL")
            class ACL(ctypes.Structure):
                _fields_ = [("revision", ctypes.c_ubyte), ("reserved", ctypes.c_ubyte),
                            ("size", ctypes.c_ushort), ("count", ctypes.c_ushort), ("reserved2", ctypes.c_ushort)]
            require(ctypes.cast(dacl, ctypes.POINTER(ACL)).contents.count == 1, "WindowsPrivateDACL")
            ace = ctypes.c_void_p()
            require(self.a.GetAce(dacl, 0, ctypes.byref(ace)), "WindowsSecurityUnknown")
            header = ctypes.string_at(ace, 8)
            require(header[0] == 0 and header[1] == 0 and int.from_bytes(header[4:8], "little") == 0x1F01FF
                    and self.a.EqualSid(ace.value + 8, self.sid), "WindowsPrivateDACL")
        finally:
            self.k.LocalFree(descriptor)

    def _open(self, path, directory=False, create=False, descriptor=None):
        class SA(ctypes.Structure):
            _fields_ = [("length", self.w.DWORD), ("descriptor", ctypes.c_void_p), ("inherit", self.w.BOOL)]
        attributes = SA(ctypes.sizeof(SA), descriptor.value if descriptor else None, False)
        access = 0x20080 if directory else (0xC0020000 if create else 0x80020000)
        sharing = 3 if directory else (0 if create else 1)  # Files deny write/delete sharing; directories deny rename/delete.
        flags = 0x200000 | (0x2000000 if directory else 0x80)
        handle = self.k.CreateFileW(str(path), access, sharing, ctypes.byref(attributes) if descriptor else None,
                                   1 if create else 3, flags, None)
        require(handle not in (None, ctypes.c_void_p(-1).value), "WindowsOpenUnknown")
        return handle

    def _parents(self, path, private=False):
        p = Path(path)
        require(p.is_absolute() and len(p.drive) == 2 and p.drive[1] == ":" and ".." not in p.parts
                and all(":" not in x for x in p.parts[1:]), "UnsafeWindowsPath")
        handles = []
        current = Path(p.anchor)
        try:
            for part in ("", *p.parts[1:-1]):
                if part:
                    current = current / part
                handle = self._open(current, True)
                handles.append(handle)
                self.metadata(handle, True)
            if private:
                self.security(handles[-1])
            return handles, p
        except BaseException:
            self.close_all(reversed(handles))
            raise

    def _descriptor(self):
        descriptor = ctypes.c_void_p()
        require(self.a.ConvertStringSecurityDescriptorToSecurityDescriptorW(
            "O:" + self.sid_text + "D:P(A;;FA;;;" + self.sid_text + ")", 1,
            ctypes.byref(descriptor), None), "WindowsCreateSecurity")
        return descriptor

    def read(self, path, maximum, private):
        parents, p = self._parents(path, private)
        handle = None
        try:
            handle = self._open(p)
            before = self.metadata(handle)
            require(before[4] <= maximum, "InputSize")
            if private:
                self.security(handle)
            data = bytearray()
            while len(data) <= maximum:
                buffer = ctypes.create_string_buffer(min(65536, maximum + 1 - len(data)))
                count = self.w.DWORD()
                require(self.k.ReadFile(handle, buffer, len(buffer), ctypes.byref(count), None), "WindowsReadUnknown")
                if not count.value:
                    break
                data.extend(buffer.raw[:count.value])
            require(before == self.metadata(handle) and len(data) == before[4] and len(data) <= maximum, "FileChanged")
            return bytes(data)
        finally:
            self.close_all(([handle] if handle is not None else []) + list(reversed(parents)))


    def create(self, path, data):
        parents, p = self._parents(path, True)
        descriptor, handle = self._descriptor(), None
        try:
            handle = self._open(p, create=True, descriptor=descriptor)
            self.metadata(handle)
            self.security(handle)  # Before the first secret byte is written.
            offset = 0
            while offset < len(data):
                block = data[offset:offset + 65536]
                count = self.w.DWORD()
                buffer = ctypes.create_string_buffer(block)
                require(self.k.WriteFile(handle, buffer, len(block), ctypes.byref(count), None)
                        and 0 < count.value <= len(block), "WindowsWriteUnknown")
                offset += count.value
            require(self.k.FlushFileBuffers(handle), "WindowsWriteUnknown")
        finally:
            self.k.LocalFree(descriptor)
            self.close_all(([handle] if handle is not None else []) + list(reversed(parents)))

    def directory(self, path):
        parents, p = self._parents(path)
        descriptor = self._descriptor()
        class SA(ctypes.Structure):
            _fields_ = [("length", self.w.DWORD), ("descriptor", ctypes.c_void_p), ("inherit", self.w.BOOL)]
        attributes = SA(ctypes.sizeof(SA), descriptor.value, False)
        handle = None
        try:
            require(self.k.CreateDirectoryW(str(p), ctypes.byref(attributes)), "WindowsDirectoryCreate")
            handle = self._open(p, True)
            self.metadata(handle, True)
            self.security(handle)
        finally:
            self.k.LocalFree(descriptor)
            self.close_all(([handle] if handle is not None else []) + list(reversed(parents)))


UNRESOLVED_FILE_HANDLES = []


def first_lf_frame(chunks):
    """FIRST_LF_FRAME_V1, independent of recv segmentation; never a second frame."""
    data = bytearray()
    for block in chunks:
        require(type(block) is bytes)
        remaining = 65 - len(data)
        prefix = block[:remaining]
        end = prefix.find(b"\n")
        if end >= 0:
            data.extend(prefix[:end + 1])
            return bytes(data), "Complete"
        data.extend(prefix)
        if len(data) == 65:
            return bytes(data), "NoLF"
        if not block:
            return bytes(data), "EarlyEOF"
    return bytes(data), "ReadTimeout"


def frame_evidence(data, state):
    return {"state": state, "count": len(data), "sha256": sha(data)}


def tcp_read(recv, readable, clock, deadline):
    chunks = []
    while clock() < deadline:
        if not readable(max(0, deadline - clock())):
            break
        try:
            block = recv(65 - sum(map(len, chunks)))
        except BlockingIOError:
            continue
        except OSError:
            data, _ = first_lf_frame(chunks)
            return data, "ReadError"
        chunks.append(block)
        data, state = first_lf_frame(chunks)
        if state != "ReadTimeout":
            return data, state
    data, _ = first_lf_frame(chunks)
    return data, "ReadTimeout"


def echo_write(send, writable, payload, clock, deadline):
    offset = 0
    while offset < len(payload) and clock() < deadline:
        if not writable(max(0, deadline - clock())):
            break
        try:
            count = send(payload[offset:])
        except BlockingIOError:
            continue
        except OSError:
            break
        if not 0 < count <= len(payload) - offset:
            break
        offset += count
    return offset == len(payload)


def empty_counters():
    return {p: {key: 0 for key in COUNTER_KEYS} for p in ("tcp", "udp")}


def counter_check(value):
    shape(value, {"tcp", "udp"})
    for item in value.values():
        shape(item, COUNTER_KEYS)
        for count in item.values():
            integer(count, 0, MAX_REQUESTS)
        require(item["total"] == sum(item[k] for k in ("pending", "matched", "wrongNonce", "foreignPeer", "readFailed", "malformed")), "CounterConservation")
        require(item["echoWritten"] + item["writeFailed"] == item["matched"], "CounterConservation")
    require(sum(x["total"] for x in value.values()) <= MAX_REQUESTS, "GlobalBudget")


class OutputQueue:
    """Nonblocking pipe/output; bounded bytes/events. It never closes sockets itself."""
    def __init__(self, fd, write=os.write):
        self.fd, self.write = fd, write
        self.pending = bytearray()
        self.events = 0

    def put(self, raw):
        require(len(raw) <= MAX_RECORD and b"\n" not in raw, "RecordSize")
        require(self.events < MAX_EVENTS and len(self.pending) + len(raw) + 1 <= MAX_OUTPUT, "RecordBackpressure")
        self.events += 1
        self.pending.extend(raw + b"\n")

    def pump(self):
        if not self.pending:
            return
        try:
            count = self.write(self.fd, self.pending)
        except BlockingIOError:
            return
        except OSError:
            raise Fault("RecordOutputLost") from None
        require(0 < count <= len(self.pending), "RecordOutputLost")
        del self.pending[:count]


class OwnedPopen(subprocess.Popen):
    """No Popen implicit polling, context-exit waiting, PID signaling or orphan queue."""
    def __del__(self):
        pass

    def forbidden(self, *args, **kwargs):
        raise Fault("OriginalWaiterOnly")

    poll = wait = communicate = kill = terminate = send_signal = forbidden
    _internal_poll = __enter__ = __exit__ = forbidden


class OriginalWaiter:
    def __init__(self, child, waitpid=os.waitpid if os.name != "nt" else None, win=None):
        self.child, self.waitpid, self.win = child, waitpid, win
        self.cached = None
        self.lost = False
        self.lock = threading.Lock()

    def observe(self):
        with self.lock:
            if self.cached:
                return self.cached.copy()
            if self.lost:
                return {"state": "Unknown", "error": "LostWaitOwnership"}
            try:
                if self.win is not None or os.name == "nt":
                    api = self.win
                    if api is None:
                        import _winapi
                        api = _winapi
                    result = api.WaitForSingleObject(self.child._handle, 0)
                    if result == 258:
                        return {"state": "Pending"}
                    require(result == 0, "NativeWaitUnknown")
                    code = api.GetExitCodeProcess(self.child._handle)
                else:
                    pid, status = self.waitpid(self.child.pid, os.WNOHANG)
                    if pid == 0:
                        return {"state": "Pending"}
                    require(pid == self.child.pid and (os.WIFEXITED(status) or os.WIFSIGNALED(status)), "NativeWaitUnknown")
                    code = os.waitstatus_to_exitcode(status)
            except ChildProcessError:
                self.lost = True
                return {"state": "Unknown", "error": "LostWaitOwnership"}
            except OSError as error:
                if error.errno == errno.ECHILD:
                    self.lost = True
                    return {"state": "Unknown", "error": "LostWaitOwnership"}
                return {"state": "Unknown", "error": "NativeWaitUnknown"}
            except Fault:
                return {"state": "Unknown", "error": "NativeWaitUnknown"}
            self.child.returncode = code  # Only the status from the original native object.
            self.cached = {"state": "Waited", "returnCode": code}
            return self.cached.copy()


class SocketCustody:
    def __init__(self, deadline, clock=time.monotonic):
        self.deadline, self.clock = deadline, clock
        self.lock = threading.Lock()
        self.handles = []
        self.results = {}
        self.releasing = set()
        self.closing = False
        self.reason = None
        self.stopped = threading.Event()
        self.closed = threading.Event()

    def publish(self, role, handle):
        with self.lock:
            admitted = not self.closing and self.clock() < self.deadline
            self.handles.append((role, handle))
        if not admitted:
            self.release(role, handle)
            raise Fault("LeaseClosed")

    def release(self, role, handle):
        with self.lock:
            if role in self.results or role in self.releasing:
                return
            self.releasing.add(role)
        try:
            handle.close()
            result = "Closed"
        except OSError:
            result = "Unknown"
        with self.lock:
            self.results[role] = result
            self.releasing.remove(role)
            if self.closing and len(self.results) == len(self.handles):
                self.closed.set()

    def close(self, reason):
        with self.lock:
            if self.closing:
                return
            self.closing, self.reason = True, reason
            handles = tuple(self.handles)
        self.stopped.set()
        for role, handle in handles:
            self.release(role, handle)
        with self.lock:
            if len(self.results) == len(self.handles):
                self.closed.set()

    def guardian(self):
        self.stopped.wait(max(0, self.deadline - self.clock()))
        self.close(self.reason or "AbsoluteDeadline")


class Receiver:
    def __init__(self, value, plan_digest, controller_id, emit, factory=socket.socket,
                 clock=time.monotonic, windows=None):
        self.plan, self.binding = value, value["binding"]
        self.plan_digest, self.controller_id = plan_digest, controller_id
        self.receiver_id = secrets.token_hex(16)
        self.emit, self.factory, self.clock = emit, factory, clock
        self.windows = os.name == "nt" if windows is None else windows
        self.seq, self.requests, self.snapshots = 0, 0, 0
        self.counters = empty_counters()
        self.sockets, self.facts = {}, []
        self.start = None
        self.custody = None

    def record(self, phase, **fields):
        self.seq += 1
        value = {"schema": "polaris-pc-lan-receiver-v1", "role": "Receiver", "phase": phase,
                 "binding": self.binding, "pcPlanSha256": self.plan_digest,
                 "sourceKind": self.plan["sourceKind"], "controllerInstanceId": self.controller_id,
                 "receiverInstanceId": self.receiver_id, "receiverSeq": self.seq, **fields}
        self.emit(canonical(value))

    def setup(self):
        if self.windows:
            require(hasattr(socket, "SO_EXCLUSIVEADDRUSE"), "ExclusiveBindUnsupported")
        self.start = self.clock()
        self.custody = SocketCustody(self.start + self.binding["lifetimeSeconds"], self.clock)
        guardian = threading.Thread(target=self.custody.guardian, name="pc-echo-deadline", daemon=True)
        guardian.start()
        try:
            for protocol in ("tcp", "udp"):
                handle = self.factory(socket.AF_INET, socket.SOCK_STREAM if protocol == "tcp" else socket.SOCK_DGRAM)
                self.custody.publish(protocol, handle)
                if self.windows:
                    handle.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
                handle.setblocking(False)
                address = (self.binding["destinationIPv4"], self.binding[protocol + "Port"])
                handle.bind(address)
                if protocol == "tcp":
                    handle.listen(4)
                require(handle.getsockname() == address, "BoundTupleMismatch")
                self.sockets[protocol] = handle
                self.facts.append({"protocol": protocol, "address": address[0], "port": address[1],
                    "socketInstanceId": secrets.token_hex(16), "tcpListening": protocol == "tcp"})
            with self.custody.lock:
                require(not self.custody.closing and self.clock() < self.custody.deadline, "LeaseClosed")
                started_ns = int(self.start * 1e9)
                self.record("Ready", sockets=self.facts, startedMonotonicNs=started_ns,
                            expiresMonotonicNs=started_ns + self.binding["lifetimeSeconds"] * 1000000000,
                            observedMonotonicNs=int(self.clock() * 1e9), counters=self.counters)
        except BaseException:
            self.custody.close("StartupFailed")
            raise
        return guardian

    def snapshot(self):
        require(self.snapshots < 24, "SnapshotBudget")
        self.snapshots += 1
        self.record("Counters", counters=self.counters, sealed=False,
                    observedMonotonicNs=int(self.clock() * 1e9))

    def request(self, protocol, payload, state, peer, send, writable, deadline):
        item = self.counters[protocol]
        require(item["pending"] > 0, "RequestNotAccepted")
        expected = self.plan["echoNonce"].encode("ascii") + (b"\n" if protocol == "tcp" else b"")
        if peer == "Unknown":
            outcome = "malformed"
        elif peer != self.binding["expectedSenderIPv4"]:
            outcome = "foreignPeer"
        elif state in ("ReadError", "ReadTimeout"):
            outcome = "readFailed"
        elif state != "Complete":
            outcome = "malformed"
        elif payload != expected:
            outcome = "wrongNonce"
        else:
            outcome = "matched"
        item["pending"] -= 1
        item[outcome] += 1
        echo = "NotAttempted"
        if outcome == "matched":
            success = echo_write(send, writable, expected, self.clock, deadline)
            item["echoWritten" if success else "writeFailed"] += 1
            echo = "Complete" if success else "Failed"
        counter_check(self.counters)
        fact = next(f for f in self.facts if f["protocol"] == protocol)
        self.record("Request", request={"protocol": protocol, "socketInstanceId": fact["socketInstanceId"],
                    "peerIPv4": peer, "classification": outcome, "frame": frame_evidence(payload, state),
                    "echoState": echo}, counters=self.counters,
                    observedMonotonicNs=int(self.clock() * 1e9))

    def begin_request(self, protocol):
        require(self.requests < MAX_REQUESTS, "GlobalBudget")
        self.requests += 1
        self.counters[protocol]["total"] += 1
        self.counters[protocol]["pending"] += 1
        return self.requests

    def finish(self, reason):
        if self.custody is None:
            return
        self.custody.close(reason)
        self.custody.closed.wait(1)
        self.record("Closed", counters=self.counters, sealed=True, stopReason=self.custody.reason,
                    sockets=[{**fact, "closeState": self.custody.results.get(fact["protocol"], "Unknown")}
                             for fact in self.facts],
                    allHandlesClosed=self.custody.closed.is_set() and
                    len(self.custody.handles) == len(self.custody.results) and
                    all(result == "Closed" for result in self.custody.results.values()),
                    observedMonotonicNs=int(self.clock() * 1e9))


class LineInput:
    def __init__(self, fd, maximum=MAX_RECORD, read=os.read):
        self.fd, self.maximum, self.read = fd, maximum, read
        self.pending, self.eof = bytearray(), False

    def drain(self):
        if not self.eof:
            try:
                block = self.read(self.fd, self.maximum + 1)
            except BlockingIOError:
                block = None
            if block == b"":
                self.eof = True
            elif block is not None:
                self.pending.extend(block)
        require(len(self.pending) <= self.maximum * 2, "PipeInputSize")
        result = []
        while b"\n" in self.pending:
            raw, _, tail = self.pending.partition(b"\n")
            require(0 < len(raw) <= self.maximum, "PipeInputSize")
            if len(result) == 8:
                break
            result.append(bytes(raw))
            self.pending = bytearray(tail)
        require(len(self.pending) <= self.maximum, "PipeInputSize")
        return result


def _wait_socket(handle, writing, seconds):
    if seconds <= 0:
        return False
    try:
        ready = select.select([] if writing else [handle], [handle] if writing else [], [], seconds)
        return bool(ready[1 if writing else 0])
    except OSError:
        return False


def receiver_loop(receiver, controls, output):
    reason = "AbsoluteDeadline"
    try:
        receiver.setup()
        while not receiver.custody.closing and receiver.clock() < receiver.custody.deadline:
            output.pump()
            for raw in controls.drain():
                command = shape(decode(raw), {"command"})["command"]
                require(command in ("snapshot", "stop"), "ControlNotAllowed")
                if command == "snapshot":
                    receiver.snapshot()
                else:
                    reason = "ControllerStop"
                    receiver.custody.close(reason)
                    break
            if receiver.custody.closing:
                reason = receiver.custody.reason
                break
            if controls.eof:
                reason = "ControlEOF"
                break
            if receiver.requests == MAX_REQUESTS:
                reason = "RequestBudget"
                break
            handles = tuple(receiver.sockets.values())
            readable = select.select(handles, [], [], min(.05, max(0, receiver.custody.deadline - receiver.clock())))[0]
            for protocol, handle in receiver.sockets.items():
                if handle not in readable or receiver.requests == MAX_REQUESTS or receiver.custody.closing:
                    continue
                if protocol == "tcp":
                    try:
                        connection, peer = handle.accept()
                    except BlockingIOError:
                        continue
                    index = receiver.begin_request(protocol)  # Admission count precedes payload read.
                    role = "connection-" + str(index)
                    receiver.custody.publish(role, connection)
                    try:
                        connection.setblocking(False)
                        deadline = min(receiver.clock() + 1, receiver.custody.deadline)
                        payload, state = tcp_read(connection.recv,
                            lambda n: _wait_socket(connection, False, n), receiver.clock, deadline)
                        receiver.request(protocol, payload, state, peer[0], connection.send,
                            lambda n: _wait_socket(connection, True, n), deadline)
                    finally:
                        receiver.custody.release(role, connection)
                else:
                    try:
                        payload, peer = handle.recvfrom(1500)
                    except BlockingIOError:
                        continue
                    except OSError as error:
                        if getattr(error, "winerror", None) == 10040:
                            receiver.begin_request(protocol)
                            receiver.request(protocol, b"", "OversizedConsumed", "Unknown",
                                             lambda _: 0, lambda _: False, receiver.custody.deadline)
                            continue
                        raise Fault("DatagramConsumptionUnknown") from None
                    receiver.begin_request(protocol)
                    state = "CapturedPrefix" if len(payload) == 1500 else "Complete"
                    receiver.request(protocol, payload, state, peer[0], lambda data: handle.sendto(data, peer),
                        lambda n: _wait_socket(handle, True, n), receiver.custody.deadline)
    except (Fault, OSError, ValueError) as error:
        reason = error.code if isinstance(error, Fault) else "ReceiverIOUnknown"
        try:
            receiver.record("Unknown", error=reason)
        except Fault:
            pass
    finally:
        try:
            receiver.finish(reason)
        except Fault:
            pass
        # Backpressure cannot hold original sockets open. A bounded tail may lose records.
        tail_end = time.monotonic() + 1
        while output.pending and time.monotonic() < tail_end:
            try:
                output.pump()
            except Fault:
                break
            time.sleep(.005)


# The runner reads the exact validated source snapshot, never reopens a reviewed path.
CHILD_RUNNER = """import sys
r = sys.stdin.buffer.raw
n = int(r.readline(16))
if not 0 < n <= 262144: raise SystemExit(2)
s = bytearray()
while len(s) < n:
    b = r.read(n - len(s))
    if not b: raise SystemExit(2)
    s.extend(b)
d = {'__name__': 'pc_lan_child'}
exec(compile(s, '<pc-lan-source-snapshot>', 'exec'), d)
d['receiver_entry'](r, bytes(s))
"""


def receiver_entry(stream, source_bytes):
    header = stream.readline(MAX_JSON + 1)
    require(header.endswith(b"\n"), "ChildHeader")
    request = shape(decode(header[:-1]), {"planBase64", "sourceManifest", "controllerInstanceId"})
    raw = raw_base64(request["planBase64"], MAX_JSON)
    value = plan(decode(raw))
    plan_digest = plan_sha(raw)
    require(value["sourceKind"] == "OperatorDeclared", "SyntheticCannotServe")
    manifest = request["sourceManifest"]
    shape(manifest, {"schema", "items"})
    require(manifest["schema"] == "polaris-pc-lan-execution-source-v1" and type(manifest["items"]) is list
            and len(manifest["items"]) == 2, "SourceInventory")
    for item, (role, path) in zip(manifest["items"], SOURCE_ITEMS):
        shape(item, {"role", "path", "sha256"})
        require(item["role"] == role and item["path"] == path, "SourceInventory")
        hex_value(item["sha256"], 64)
    require(manifest["items"][0]["sha256"] == sha(source_bytes) and
            digest("polaris-pc-lan-execution-source-v1", canonical(manifest)) == value["binding"]["receiverSourceSha256"], "SourceBinding")
    hex_value(request["controllerInstanceId"], 32)
    # No further buffered read after the startup header; pipe controls are nonblocking.
    require(stat.S_ISFIFO(os.fstat(0).st_mode) and stat.S_ISFIFO(os.fstat(1).st_mode), "OriginalPipesRequired")
    os.set_blocking(0, False)
    os.set_blocking(1, False)
    output = OutputQueue(1)
    receiver = Receiver(value, plan_digest, request["controllerInstanceId"], output.put)
    receiver_loop(receiver, LineInput(0, 1024), output)


def execution_sources():
    root = Path(__file__).absolute().parent
    items = {path: read_file(root / path, MAX_SOURCE, False) for _, path in SOURCE_ITEMS}
    manifest, source_hash = source_manifest(items)
    return items, manifest, source_hash


def _wrapper(controller_id, sequence, source_kind, raw):
    return {"schema": "polaris-pc-lan-controller-v1", "role": "Controller", "phase": "ReceiverRecord",
            "controllerInstanceId": controller_id, "eventSeq": sequence, "sourceKind": source_kind,
            "receiverRaw": base64.b64encode(raw).decode("ascii"), "receiverRawSha256": sha(raw)}


def controller_run(raw_plan, sources, manifest, sink, console_input=0):
    """Future root driver seam. No caller boolean authorizes execution through the CLI."""
    value = plan(decode(raw_plan))
    require(value["sourceKind"] == "OperatorDeclared", "SyntheticCannotServe")
    actual_manifest, source_hash = source_manifest(sources)
    require(plan_sha(raw_plan) and actual_manifest == manifest and
            source_hash == value["binding"]["receiverSourceSha256"], "SourceBinding")
    require(isinstance(sink, OutputQueue), "NonblockingOutputRequired")
    require(stat.S_ISFIFO(os.fstat(sink.fd).st_mode), "RecordPipeRequired")
    # Capability/setup failures happen before the original child is created.
    os.set_blocking(sink.fd, False)
    os.set_blocking(console_input, False)
    if os.name != "nt":
        require(signal.getsignal(signal.SIGCHLD) == signal.SIG_DFL, "ChildWaitDisposition")
    require(threading.current_thread() is threading.main_thread(), "ControllerMainThreadRequired")
    controller_id = secrets.token_hex(16)
    header = canonical({"planBase64": base64.b64encode(raw_plan).decode("ascii"),
                        "sourceManifest": manifest, "controllerInstanceId": controller_id})
    startup = str(len(sources["lan.py"])).encode("ascii") + b"\n" + sources["lan.py"] + header + b"\n"
    require(len(startup) <= MAX_OUTPUT, "ChildStartupSize")
    child = OwnedPopen.__new__(OwnedPopen)
    debt = {"child": child, "waiter": None, "records": []}
    with CHILD_ADMISSION:
        require(not UNRESOLVED_CHILDREN, "PriorChildUnresolved")
        UNRESOLVED_CHILDREN.append(debt)
    interrupted = threading.Event()
    old_interrupt = signal.signal(signal.SIGINT, lambda *_: interrupted.set())
    # Preserve this same object even if construction or subsequent pipe publication fails.
    try:
        child.__init__([sys.executable, "-I", "-S", "-c", CHILD_RUNNER], stdin=subprocess.PIPE,
                       stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0,
                       close_fds=True, env={k: os.environ[k] for k in ("SystemRoot",) if k in os.environ})
    except BaseException:
        signal.signal(signal.SIGINT, old_interrupt)
        raise  # The retained partial construction is Unknown, never a fresh-start permit.
    waiter = OriginalWaiter(child)
    debt["waiter"] = waiter
    writer, reader, console = None, None, None
    records, sequence, stop_sent, error_sent = debt["records"], 0, False, False
    end = time.monotonic() + value["binding"]["lifetimeSeconds"] + 3

    def record(phase, **fields):
        nonlocal sequence
        require(sequence < MAX_EVENTS, "RecordBudget")
        sequence += 1
        item = {"schema": "polaris-pc-lan-controller-v1", "role": "Controller", "phase": phase,
                "controllerInstanceId": controller_id, "eventSeq": sequence,
                "sourceKind": value["sourceKind"], **fields}
        encoded = canonical(item)
        records.append(encoded)
        try:
            sink.put(encoded)
        except (Fault, OSError):
            close_control()

    def original_record(raw):
        nonlocal sequence
        sequence += 1
        require(sequence <= MAX_EVENTS, "RecordBudget")
        item = canonical(_wrapper(controller_id, sequence, value["sourceKind"], raw))
        records.append(item)
        sink.put(item)

    def close_control():
        if not child.stdin.closed:
            child.stdin.close()

    def stop(reason):
        nonlocal stop_sent
        if stop_sent:
            return
        stop_sent = True
        try:
            record("StopRequested", reason=reason)
            require(writer is not None, "ControlUnavailable")
            writer.put(canonical({"command": "stop"}))
        except (Fault, OSError):
            close_control()

    try:
        try:
            writer = OutputQueue(child.stdin.fileno())
            reader = LineInput(child.stdout.fileno())
            os.set_blocking(child.stdin.fileno(), False)
            os.set_blocking(child.stdout.fileno(), False)
            console = LineInput(console_input, 1024)
            writer.pending.extend(startup)
        except (Fault, OSError, ValueError):
            close_control()
            error_sent = True
            try:
                record("Unknown", error="ControllerIOUnknown")
            except (Fault, OSError):
                pass
        while True:
            try:
                if interrupted.is_set():
                    stop("ControllerInterrupted")
                sink.pump()
                if writer is not None and not child.stdin.closed:
                    writer.pump()
                for raw in reader.drain() if reader is not None else ():
                    original_record(raw)
                for raw in console.drain() if console is not None else ():
                    command = shape(decode(raw), {"command"})["command"]
                    require(command in ("snapshot", "stop"), "ControlNotAllowed")
                    if command == "stop":
                        stop("ControllerStop")
                    else:
                        writer.put(raw)
                if console is not None and console.eof:
                    stop("ControlEOF")
                if time.monotonic() >= end:
                    stop("ControllerDeadline")
            except KeyboardInterrupt:
                stop("ControllerInterrupted")
            except (Fault, OSError, ValueError):
                # EOF is the fallback stop channel, never a PID signal.
                close_control()
                if not error_sent:
                    error_sent = True
                    try:
                        record("Unknown", error="ControllerIOUnknown")
                    except (Fault, OSError):
                        pass
            state = waiter.observe()
            if state["state"] == "Waited":
                # Consume remaining original pipe bytes after actual wait, no Popen communicate.
                tail_end = time.monotonic() + 1
                try:
                    require(reader is not None, "ControlUnavailable")
                    while not reader.eof and time.monotonic() < tail_end:
                        for raw in reader.drain():
                            original_record(raw)
                        sink.pump()
                        if not reader.eof:
                            time.sleep(.005)
                    while reader.eof and b"\n" in reader.pending:
                        for raw in reader.drain():
                            original_record(raw)
                    require(reader.eof and not reader.pending, "TruncatedReceiverRecord")
                except (Fault, OSError, ValueError):
                    try:
                        record("Unknown", error="ControllerIOUnknown")
                    except (Fault, OSError):
                        pass
                try:
                    record("ChildWait", childWait=state)
                    while sink.pending and time.monotonic() < tail_end:
                        sink.pump()
                        if sink.pending:
                            time.sleep(.005)
                except (Fault, OSError):
                    pass
                return records
            if state["state"] == "Unknown" and not error_sent:
                error_sent = True
                try:
                    record("ChildWait", childWait=state)
                except (Fault, OSError):
                    pass
                close_control()
            if time.monotonic() >= end + 1:
                close_control()  # Stop bytes may never reach a blocked receiver; EOF stays exact.
            # Unknown returns a record promptly but this same foreground custodian stays alive.
            time.sleep(.01)
    finally:
        # No normal exception may drop an unresolved original process object.
        if waiter.cached is not None:
            child.stdin.close()
            child.stdout.close()
            with CHILD_ADMISSION:
                UNRESOLVED_CHILDREN.remove(debt)
        signal.signal(signal.SIGINT, old_interrupt)


UNRESOLVED_CHILDREN = []
CHILD_ADMISSION = threading.Lock()


RECEIVER_COMMON = {"schema", "role", "phase", "binding", "pcPlanSha256", "sourceKind",
                   "controllerInstanceId", "receiverInstanceId", "receiverSeq"}
RECEIVER_FIELDS = {
    "Ready": {"sockets", "startedMonotonicNs", "expiresMonotonicNs", "observedMonotonicNs", "counters"},
    "Counters": {"counters", "sealed", "observedMonotonicNs"},
    "Request": {"request", "counters", "observedMonotonicNs"},
    "Closed": {"counters", "sealed", "stopReason", "sockets", "allHandlesClosed", "observedMonotonicNs"},
    "Unknown": {"error"},
}
ERRORS = {"RecordBackpressure", "RecordOutputLost", "RecordSize", "SnapshotBudget", "ControlNotAllowed",
          "PipeInputSize", "ReceiverIOUnknown", "DatagramConsumptionUnknown", "LeaseClosed",
          "ExclusiveBindUnsupported", "BoundTupleMismatch", "StartupFailed", "InvalidShape", "InvalidJSON"}
STOP_REASONS = ERRORS | {"AbsoluteDeadline", "ControllerStop", "ControlEOF", "RequestBudget"}
CODEC_ERRORS = {"InvalidShape", "InvalidDiscriminator", "InvalidBase64", "InvalidJSON", "InvalidIPv4",
                "InputSize", "DuplicateKey", "NonIntegerNumber", "NestingLimit", "StringLimit",
                "ArrayLimit", "ObjectLimit", "DuplicatePort", "NonceBinding", "NonCanonicalPlan",
                "SourceInventory", "SourceSize", "SourceBinding", "RawRecordCodec", "OriginalRawDigest",
                "OriginalChildBinding", "OriginalStreamFraming", "SocketBinding", "LeaseBinding"}


def socket_facts(value, closed=False):
    require(type(value) is list and len(value) <= 2)
    protocols = []
    for item in value:
        shape(item, {"protocol", "address", "port", "socketInstanceId", "tcpListening"}
              | ({"closeState"} if closed else set()))
        protocols.append(choice(item["protocol"], ("tcp", "udp")))
        ipv4(item["address"])
        integer(item["port"], 49152, 65535)
        hex_value(item["socketInstanceId"], 32)
        require(type(item["tcpListening"]) is bool and item["tcpListening"] == (item["protocol"] == "tcp"))
        if closed:
            choice(item["closeState"], ("Closed", "Unknown"))
    require(len(set(protocols)) == len(value))


def receiver_record(raw):
    item = decode(raw, MAX_RECORD)
    require(type(item) is dict)
    phase = choice(item.get("phase"), RECEIVER_FIELDS)
    fields = RECEIVER_COMMON | RECEIVER_FIELDS[phase]
    # Absent business observation time is an explicit incomplete fact, never invented.
    if phase in ("Request", "Counters") and "observedMonotonicNs" not in item:
        fields = fields - {"observedMonotonicNs"}
    shape(item, fields)
    require(canonical(item) == raw and item["schema"] == "polaris-pc-lan-receiver-v1"
            and item["role"] == "Receiver", "RawRecordCodec")
    binding(item["binding"])
    hex_value(item["pcPlanSha256"], 64)
    for key in ("controllerInstanceId", "receiverInstanceId"):
        hex_value(item[key], 32)
    integer(item["receiverSeq"], 1, MAX_EVENTS)
    require(item["sourceKind"] in ("Synthetic", "OperatorDeclared"))
    if phase == "Unknown":
        choice(item["error"], ERRORS)
        return item
    counter_check(item["counters"])
    if "observedMonotonicNs" in item:
        integer(item["observedMonotonicNs"])
    if phase in ("Ready", "Closed"):
        socket_facts(item["sockets"], phase == "Closed")
        for fact in item["sockets"]:
            require(fact["address"] == item["binding"]["destinationIPv4"] and
                    fact["port"] == item["binding"][fact["protocol"] + "Port"], "SocketBinding")
    if phase == "Ready":
        require(len(item["sockets"]) == 2)
        require(sum(x["total"] for x in item["counters"].values()) == 0)
        start, end = integer(item["startedMonotonicNs"]), integer(item["expiresMonotonicNs"])
        require(start <= item["observedMonotonicNs"] < end and
                abs(end - start - item["binding"]["lifetimeSeconds"] * 1000000000) <= 1, "LeaseBinding")
    if phase in ("Counters", "Closed"):
        require(type(item["sealed"]) is bool and item["sealed"] == (phase == "Closed"))
    if phase == "Closed":
        choice(item["stopReason"], STOP_REASONS)
        require(type(item["allHandlesClosed"]) is bool)
    if phase == "Request":
        request = shape(item["request"], {"protocol", "socketInstanceId", "peerIPv4", "classification", "frame", "echoState"})
        require(request["protocol"] in ("tcp", "udp"))
        hex_value(request["socketInstanceId"], 32)
        if request["peerIPv4"] != "Unknown":
            require(str(ipaddress.IPv4Address(request["peerIPv4"])) == request["peerIPv4"])
        choice(request["classification"], ("matched", "wrongNonce", "foreignPeer", "readFailed", "malformed"))
        frame = shape(request["frame"], {"state", "count", "sha256"})
        require(frame["state"] in ("Complete", "NoLF", "EarlyEOF", "ReadTimeout", "ReadError", "CapturedPrefix", "OversizedConsumed"))
        integer(frame["count"], 0, 65 if request["protocol"] == "tcp" else 1500)
        hex_value(frame["sha256"], 64)
        require(request["echoState"] in ("NotAttempted", "Complete", "Failed"))
        require((request["classification"] == "matched") == (request["echoState"] != "NotAttempted"))
    return item


def wait_union(value):
    require(type(value) is dict)
    choice(value.get("state"), ("Pending", "Waited", "Unknown"))
    if value["state"] == "Waited":
        shape(value, {"state", "returnCode"})
        integer(value["returnCode"], -(1 << 31), (1 << 32) - 1)
    elif value["state"] == "Unknown":
        shape(value, {"state", "error"})
        require(value["error"] in ("LostWaitOwnership", "NativeWaitUnknown"))
    else:
        shape(value, {"state"})


def controller_record(raw):
    item = decode(raw, MAX_RECORD)
    common = {"schema", "role", "phase", "controllerInstanceId", "eventSeq", "sourceKind"}
    fields = {"ReceiverRecord": {"receiverRaw", "receiverRawSha256"}, "StopRequested": {"reason"},
              "ChildWait": {"childWait"}, "Unknown": {"error"}}
    require(type(item) is dict)
    choice(item.get("phase"), fields)
    shape(item, common | fields[item["phase"]])
    require(canonical(item) == raw and item["schema"] == "polaris-pc-lan-controller-v1" and item["role"] == "Controller")
    hex_value(item["controllerInstanceId"], 32)
    integer(item["eventSeq"], 1, MAX_EVENTS)
    require(item["sourceKind"] in ("Synthetic", "OperatorDeclared"))
    if item["phase"] == "ReceiverRecord":
        child_raw = raw_base64(item["receiverRaw"])
        require(sha(child_raw) == item["receiverRawSha256"], "OriginalRawDigest")
        child = receiver_record(child_raw)
        require(child["controllerInstanceId"] == item["controllerInstanceId"], "OriginalChildBinding")
    elif item["phase"] == "StopRequested":
        require(item["reason"] in ("ControllerStop", "ControlEOF", "ControllerDeadline", "ControllerInterrupted"))
    elif item["phase"] == "ChildWait":
        wait_union(item["childWait"])
    else:
        require(item["error"] == "ControllerIOUnknown")
    return item


def bundle_value(raw_plan, records, witnesses, cases):
    value = plan(decode(raw_plan))
    require(type(records) is list and len(records) <= MAX_EVENTS)
    require(type(witnesses) is list and len(witnesses) <= 32 and
            type(cases) is list and len(cases) <= len(CASE_IDS))
    synthetic = value["sourceKind"] == "Synthetic"
    entries = []
    for raw in records:
        item = controller_record(raw)
        synthetic |= item["sourceKind"] == "Synthetic"
        if item["phase"] == "ReceiverRecord":
            synthetic |= receiver_record(raw_base64(item["receiverRaw"]))["sourceKind"] == "Synthetic"
        entries.append({"raw": base64.b64encode(raw).decode("ascii"), "sha256": sha(raw)})
    for witness in witnesses:
        public_witness(witness)
    return {"schema": "polaris-pc-lan-bundle-v1", "pcPlanSha256": plan_sha(raw_plan),
            "sourceKind": "Synthetic" if synthetic else "OperatorDeclared", "records": entries,
            "publicWitnesses": witnesses, "cases": cases}


def counter_transition(previous, item):
    """The serial producer publishes completed Request results; other phases cannot mint them."""
    current = item["counters"]
    if item["phase"] == "Request":
        request = item["request"]
        require(not any(c["pending"] for c in previous.values()), "RequestAfterUnfinishedAccept")
        expected = {protocol: counters.copy() for protocol, counters in previous.items()}
        counts = expected[request["protocol"]]
        counts["total"] += 1
        counts[request["classification"]] += 1
        if request["classification"] == "matched":
            counts["echoWritten" if request["echoState"] == "Complete" else "writeFailed"] += 1
        require(current == expected, "RequestCounterDelta")
        return False
    for protocol in ("tcp", "udp"):
        before, after = previous[protocol], current[protocol]
        require(all(after[key] == before[key] for key in COUNTER_KEYS - {"total", "pending"}),
                "UnattributedCounterDelta")
        require(after["total"] - before["total"] == after["pending"] - before["pending"] >= 0,
                "UnattributedCounterDelta")
    return any(c["pending"] for c in current.values())


def business_window(item, ready):
    observed = item.get("observedMonotonicNs")
    if observed is None:
        return False
    require(ready["startedMonotonicNs"] <= observed < ready["expiresMonotonicNs"], "BusinessOutsideLease")
    return True


def verify(raw_plan, value):
    """Shape/consistency only. No label, hash, signature-looking field authenticates origin."""
    incomplete = []
    try:
        shape(value, {"schema", "pcPlanSha256", "sourceKind", "records", "publicWitnesses", "cases"})
        require(value["schema"] == "polaris-pc-lan-bundle-v1" and value["pcPlanSha256"] == plan_sha(raw_plan), "PlanBinding")
        p = plan(decode(raw_plan))
        require(value["sourceKind"] in ("Synthetic", "OperatorDeclared"))
        synthetic = p["sourceKind"] == "Synthetic" or value["sourceKind"] == "Synthetic"
        require(type(value["records"]) is list and len(value["records"]) <= MAX_EVENTS)
        ready = closed = waited = None
        wait_identity_lost = False
        snapshots, requests = {}, []
        controller_id = receiver_id = None
        receiver_seq, controller_seq, observed = 0, 0, 0
        previous = empty_counters()
        ready_sha = None
        sockets = {}
        for entry in value["records"]:
            shape(entry, {"raw", "sha256"})
            raw = raw_base64(entry["raw"])
            require(sha(raw) == entry["sha256"], "OriginalRawDigest")
            item = controller_record(raw)
            require(item["eventSeq"] == controller_seq + 1, "EventOrder")
            controller_seq += 1
            controller_id = controller_id or item["controllerInstanceId"]
            require(item["controllerInstanceId"] == controller_id, "ControllerIdentity")
            synthetic |= item["sourceKind"] == "Synthetic"
            if item["phase"] == "Unknown":
                incomplete.append("ControllerUnknown")
            elif item["phase"] == "ChildWait":
                state = item["childWait"]
                require(waited is None, "DuplicateWait")
                if state["state"] == "Waited":
                    require(not wait_identity_lost, "WaitAfterLostOwnership")
                    waited = state
                else:
                    incomplete.append("NativeWaitUnknown")
                    wait_identity_lost |= state.get("error") == "LostWaitOwnership"
            elif item["phase"] == "StopRequested":
                require(waited is None, "ControlAfterWait")
            elif item["phase"] == "ReceiverRecord":
                require(waited is None, "RecordAfterWait")
                child_raw = raw_base64(item["receiverRaw"])
                child = receiver_record(child_raw)
                require(child["receiverSeq"] == receiver_seq + 1, "ReceiverOrder")
                receiver_seq += 1
                require(child["binding"] == p["binding"] and child["pcPlanSha256"] == value["pcPlanSha256"], "ReceiverBinding")
                receiver_id = receiver_id or child["receiverInstanceId"]
                require(child["receiverInstanceId"] == receiver_id, "ReceiverIdentity")
                synthetic |= child["sourceKind"] == "Synthetic"
                if child["phase"] == "Unknown":
                    incomplete.append("ReceiverUnknown")
                    continue
                require(closed is None, "RecordAfterClose")
                if "observedMonotonicNs" in child:
                    require(child["observedMonotonicNs"] >= observed, "LocalClockOrder")
                    observed = child["observedMonotonicNs"]
                if child["phase"] == "Ready":
                    require(ready is None and receiver_seq == 1, "ReadyOrder")
                    ready, ready_sha = child, sha(child_raw)
                    sockets = {x["protocol"]: x for x in child["sockets"]}
                    require(sockets["tcp"]["socketInstanceId"] != sockets["udp"]["socketInstanceId"], "SocketIdentity")
                else:
                    require(ready is not None, "MissingReady")
                    if counter_transition(previous, child):
                        incomplete.append("UnfinishedAcceptedRequest")
                if child["phase"] == "Counters":
                    snapshots[item["eventSeq"]] = child
                elif child["phase"] == "Request":
                    if not business_window(child, ready):
                        incomplete.append("MissingBusinessTime")
                    request = child["request"]
                    require(request["socketInstanceId"] == sockets[request["protocol"]]["socketInstanceId"], "SocketIdentity")
                    expected = p["echoNonce"].encode("ascii") + (b"\n" if request["protocol"] == "tcp" else b"")
                    if request["classification"] == "matched":
                        require(request["peerIPv4"] == p["binding"]["expectedSenderIPv4"] and
                                request["frame"] == frame_evidence(expected, "Complete"), "MatchedFrame")
                    requests.append((item["eventSeq"], request))
                elif child["phase"] == "Closed":
                    closed = child
                    require([{k: x[k] for k in x if k != "closeState"} for x in child["sockets"]] == ready["sockets"], "ClosedSocketIdentity")
                    if not child["allHandlesClosed"] or any(x["closeState"] != "Closed" for x in child["sockets"]) or any(
                            x["pending"] for x in child["counters"].values()):
                        incomplete.append("OriginalCloseUnknown")
                previous = child["counters"]
        if ready is None or closed is None or waited is None:
            incomplete.append("MissingOriginalLifecycle")
        elif waited.get("state") != "Waited" or waited.get("returnCode") != 0:
            incomplete.append("ReceiverExitNotNormal")
        require(type(value["publicWitnesses"]) is list and len(value["publicWitnesses"]) <= 32)
        witnesses = {}
        for item in value["publicWitnesses"]:
            public_witness(item)
            require(item["attemptSeq"] not in witnesses, "DuplicateAttempt")
            witnesses[item["attemptSeq"]] = item
        require(type(value["cases"]) is list and len(value["cases"]) <= len(CASE_IDS))
        full_case_inventory = len(value["cases"]) == len(CASE_IDS) and all(
            type(case) is dict for case in value["cases"]) and {
                case.get("caseId") for case in value["cases"]} == set(CASE_IDS)
        seen_cases, seen_attempts, used_requests = set(), set(), set()
        last_snapshot = last_attempt = 0
        for case in value["cases"]:
            shape(case, {"caseId", "snapshotSeqs", "attemptSeqs"})
            cid = case["caseId"]
            require(cid in CASE_IDS and cid not in seen_cases, "CaseIdentity")
            seen_cases.add(cid)
            require(type(case["snapshotSeqs"]) is list and len(case["snapshotSeqs"]) == 4 and
                    type(case["attemptSeqs"]) is list and len(case["attemptSeqs"]) == 3)
            ss, aa = case["snapshotSeqs"], case["attemptSeqs"]
            for seq in ss:
                integer(seq, 1, MAX_EVENTS)
            for seq in aa:
                integer(seq, 1, 32)
            require(ss == sorted(set(ss)) and ss[0] > last_snapshot and aa == sorted(set(aa))
                    and aa[0] > last_attempt and not seen_attempts.intersection(aa), "CaseOrder")
            if full_case_inventory and ss[0] in snapshots and (not last_snapshot or last_snapshot in snapshots):
                baseline = snapshots[last_snapshot]["counters"] if last_snapshot else empty_counters()
                require(snapshots[ss[0]]["counters"] == baseline, "NoiseBetweenCases")
            last_snapshot = ss[-1]
            last_attempt = aa[-1]
            seen_attempts.update(aa)
            if any(seq not in snapshots for seq in ss) or any(seq not in witnesses for seq in aa) or ready is None:
                incomplete.append("MissingCaseOriginals")
                continue
            for seq in ss:
                if not business_window(snapshots[seq], ready):
                    incomplete.append("MissingBusinessTime")
            protocol = cid.split("-", 1)[0]
            negative = {"wrong-nonce": "WrongNonce", "foreign-peer": "ForeignPeer",
                        "wrong-port-admission": "WrongPortAdmission"}[cid[len(protocol) + 1:]]
            approved = {"address": p["binding"]["destinationIPv4"], "port": p["binding"][protocol + "Port"], "protocol": protocol}
            expected = p["echoNonce"].encode("ascii") + (b"\n" if protocol == "tcp" else b"")
            byte_fact = {"kind": "Bytes", "count": len(expected), "sha256": sha(expected)}
            for index, (left, right, attempt) in enumerate(zip(ss, ss[1:], aa)):
                witness = witnesses[attempt]
                kind = negative if index == 1 else "Positive"
                require(witness["pcRunId"] == p["binding"]["runId"] and witness["pcPlanSha256"] == value["pcPlanSha256"]
                        and witness["readyReceiptSha256"] == ready_sha and witness["receiverInstanceId"] == receiver_id
                        and witness["socketInstanceId"] == sockets[protocol]["socketInstanceId"] and
                        witness["protocol"] == protocol and witness["caseId"] == cid and witness["caseKind"] == kind
                        and witness["approvedTuple"] == approved, "ProbeBinding")
                if witness["rootNativeWitness"]["kind"] != "Reference":
                    incomplete.append("MissingRootNativeOriginalReference")
                increments = {p: {k: 0 for k in COUNTER_KEYS} for p in ("tcp", "udp")}
                between_entries = [(seq, r) for seq, r in requests if left < seq < right]
                between = [r for _, r in between_entries]
                used_requests.update(seq for seq, _ in between_entries)
                if kind == "WrongPortAdmission":
                    other = "udp" if protocol == "tcp" else "tcp"
                    require(witness["requestedTuple"] == {**approved, "port": p["binding"][other + "Port"]}
                            and witness["sent"] == {"kind": "NotSent"} and witness["returned"] == {"kind": "NotReceived"}
                            and witness["outcome"] == "RejectedBeforeOutbound", "WrongPortAdmission")
                    require(not between, "UnexpectedNegativeWire")
                else:
                    require(witness["requestedTuple"] == approved, "TargetMismatch")
                    if len(between) != 1:
                        incomplete.append("NegativeOrControlNotObserved")
                        continue
                    request = between[0]
                    classification = "matched" if kind == "Positive" else ("wrongNonce" if kind == "WrongNonce" else "foreignPeer")
                    require(request["protocol"] == protocol and request["socketInstanceId"] == sockets[protocol]["socketInstanceId"]
                            and request["classification"] == classification, "RequestAttribution")
                    increments[protocol]["total"] = increments[protocol][classification] = 1
                    if kind == "Positive":
                        require(witness["sent"] == byte_fact and witness["returned"] == byte_fact and witness["outcome"] == "ExactEcho"
                                and request["echoState"] == "Complete", "PositiveControl")
                        increments[protocol]["echoWritten"] = 1
                    else:
                        require(witness["sent"]["kind"] == "Bytes" and witness["returned"] == {"kind": "NotReceived"}
                                and witness["outcome"] == "NoEcho" and request["echoState"] == "NotAttempted"
                                and request["frame"] == {"state": "Complete", "count": witness["sent"]["count"],
                                                          "sha256": witness["sent"]["sha256"]}, "NegativeFrame")
                        if kind == "WrongNonce":
                            require(witness["sent"]["sha256"] != byte_fact["sha256"] and
                                    request["peerIPv4"] == p["binding"]["expectedSenderIPv4"], "WrongNonce")
                        else:
                            require(request["peerIPv4"] not in ("Unknown", p["binding"]["expectedSenderIPv4"]), "ForeignPeerMissing")
                for proto in ("tcp", "udp"):
                    for key in COUNTER_KEYS:
                        require(snapshots[right]["counters"][proto][key] - snapshots[left]["counters"][proto][key]
                                == increments[proto][key], "UnexplainedCounterDelta")
        if seen_cases != set(CASE_IDS):
            incomplete.append("MissingBatchCases")
        if seen_cases == set(CASE_IDS) and not incomplete:
            require(used_requests == {seq for seq, _ in requests}, "UnboundReceiverRequest")
            require(last_snapshot in snapshots and closed is not None and
                    snapshots[last_snapshot]["counters"] == closed["counters"], "NoiseAfterCases")
        require(set(witnesses) == seen_attempts, "UnboundProbe")
        if incomplete:
            return claims("Incomplete", sorted(set(incomplete)))
        return claims("LogicOnly" if synthetic else "EvidenceConsistentUnverified")
    except (Fault, ValueError, KeyError, TypeError, OverflowError, ipaddress.AddressValueError):
        return claims("Invalid", ("StructureOrBindingContradiction",))


def root_coordinated_execution(_inputs):
    """Pending root batch-driver ABI: cannot authenticate a JSON self-declaration."""
    raise Fault("ExecutionNotFrozen")


def main(argv=None):
    class Parser(argparse.ArgumentParser):
        def error(self, _message):
            # Do not reproduce operator paths, arbitrary arguments or secret values.
            self.exit(2, "InvalidCLI\n")
    parser = Parser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--input", required=True)
    prepare.add_argument("--output-directory", required=True)
    commands.add_parser("source-hash")
    serve = commands.add_parser("serve")
    serve.add_argument("--plan", required=True)
    serve.add_argument("--execution-input", required=True)
    collect = commands.add_parser("collect")
    for key in ("plan", "records", "witnesses", "cases", "output"):
        collect.add_argument("--" + key, required=True)
    check = commands.add_parser("verify")
    check.add_argument("--plan", required=True)
    check.add_argument("--bundle", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "source-hash":
            _, manifest, source_hash = execution_sources()
            result = {**claims("EvidenceConsistentUnverified"),
                      "schema": "polaris-pc-lan-source-observation-v1", "manifest": manifest,
                      "receiverSourceSha256": source_hash, "sourceAuthentication": "NotProvided",
                      "executionReady": False}
        elif args.command == "prepare":
            value = plan(decode(read_file(args.input)))
            raw = canonical(value)
            _, _, observed_source = execution_sources()
            require(observed_source == value["binding"]["receiverSourceSha256"], "SourceBinding")
            create_directory(args.output_directory)
            create_file(Path(args.output_directory) / "plan.json", raw)
            result = {**claims("LogicOnly" if value["sourceKind"] == "Synthetic" else "EvidenceConsistentUnverified"),
                      "schema": "polaris-pc-lan-prepared-v1", "phase": "PreparedOnly",
                      "pcPlanSha256": plan_sha(raw), "binding": value["binding"],
                      "sourceKind": value["sourceKind"]}
        elif args.command == "serve":
            root_coordinated_execution(args.execution_input)
        elif args.command == "collect":
            raw_plan = read_file(args.plan)
            stream = read_file(args.records, 4 * MAX_OUTPUT)
            records = stream.splitlines()
            require(stream == b"".join(x + b"\n" for x in records), "OriginalStreamFraming")
            witnesses = decode(read_file(args.witnesses))
            cases = decode(read_file(args.cases))
            value = bundle_value(raw_plan, records, witnesses, cases)
            result = verify(raw_plan, value)
            if result["verdict"] != "Invalid":
                create_file(args.output, canonical(value))
        else:
            result = verify(read_file(args.plan), decode(read_file(args.bundle, 4 * MAX_OUTPUT), 4 * MAX_OUTPUT))
        sys.stdout.buffer.write(canonical(result) + b"\n")
        return 0 if result.get("verdict") in ("LogicOnly", "EvidenceConsistentUnverified") else 2
    except (Fault, OSError, ValueError) as error:
        code = error.code if isinstance(error, Fault) else "IOOrCodecUnknown"
        verdict = "Invalid" if isinstance(error, Fault) and code in CODEC_ERRORS else "Incomplete"
        sys.stdout.buffer.write(canonical(claims(verdict, (code,))) + b"\n")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
