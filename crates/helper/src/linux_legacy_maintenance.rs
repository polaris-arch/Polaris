//! Fixed Linux legacy retirement. Old objects are data and stop-scope evidence,
//! never executable authority. No daemon flag, IPC, old CLI or rollback exists.
use crate::protected_install_path::ProtectedPath;
use polaris_helper_proto::core_manifest::{CoreId, InstalledCoreManifest};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{
    DirBuilderExt, FileExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SUPPORT: &str = "/var/lib/polaris";
const INSTALL: &str = "/usr/local/lib/polaris";
const HELPER: &str = "/usr/local/lib/polaris/helper";
const CORE: &str = "/usr/local/lib/polaris/core";
const UNIT_PARENT: &str = "/etc/systemd/system";
const UNIT: &str = "/etc/systemd/system/polaris-helper.service";
const SERVICE: &str = "polaris-helper.service";
const KIND: &str = "polaris-linux-legacy-forward-v1";
const JOURNAL_LIMIT: u64 = 32 * 1024;
const IMAGE_LIMIT: u64 = 512 * 1024 * 1024;
const OUTPUT_LIMIT: u64 = 1024 * 1024;
const KEYS: [(&str, &str); 9] = [
    ("org.gnome.system.proxy.http", "host"),
    ("org.gnome.system.proxy.http", "port"),
    ("org.gnome.system.proxy.http", "enabled"),
    ("org.gnome.system.proxy.https", "host"),
    ("org.gnome.system.proxy.https", "port"),
    ("org.gnome.system.proxy.socks", "host"),
    ("org.gnome.system.proxy.socks", "port"),
    ("org.gnome.system.proxy", "ignore-hosts"),
    ("org.gnome.system.proxy", "mode"),
];

fn rejected(reason: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason.into())
}
fn require(value: bool, reason: &'static str) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(rejected(reason))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Object {
    dev: u64,
    ino: u64,
}
impl Object {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }
    fn text(self) -> String {
        format!("{}:{}", self.dev, self.ino)
    }
    fn parse(value: &str) -> io::Result<Self> {
        let (dev, ino) = value
            .split_once(':')
            .ok_or_else(|| rejected("invalid object binding"))?;
        let value = Self {
            dev: decimal(dev)?,
            ino: decimal(ino)?,
        };
        require(value.ino > 0, "zero inode binding")?;
        Ok(value)
    }
}
fn decimal(value: &str) -> io::Result<u64> {
    require(
        !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
        "invalid decimal",
    )?;
    value.parse().map_err(|_| rejected("decimal overflow"))
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn read_held(file: &File, limit: u64) -> io::Result<Vec<u8>> {
    let before = file.metadata()?;
    require(
        before.is_file() && before.len() <= limit,
        "invalid bounded regular object",
    )?;
    let mut output = vec![0; before.len() as usize];
    file.read_exact_at(&mut output, 0)?;
    let after = file.metadata()?;
    require(
        Object::of(&before) == Object::of(&after)
            && before.len() == after.len()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "held object changed",
    )?;
    Ok(output)
}
fn hash(file: &File) -> io::Result<String> {
    use sha2::Digest;
    let before = file.metadata()?;
    require(
        before.is_file() && before.len() > 0 && before.len() <= IMAGE_LIMIT,
        "invalid image size",
    )?;
    let mut state = sha2::Sha256::new();
    let mut position = 0;
    let mut buffer = [0; 64 * 1024];
    while position < before.len() {
        let length = file.read_at(&mut buffer, position)?;
        require(length > 0, "image truncated")?;
        state.update(&buffer[..length]);
        position += length as u64;
    }
    let after = file.metadata()?;
    require(
        Object::of(&before) == Object::of(&after)
            && before.len() == after.len()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.mtime() == after.mtime()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "image changed",
    )?;
    Ok(hex::encode(state.finalize()))
}
fn nonce() -> io::Result<String> {
    let mut value = [0u8; 12];
    File::open("/dev/urandom")?.read_exact(&mut value)?;
    Ok(hex::encode(value))
}
fn open_regular(path: &Path, uid: u32) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK | nix::libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    require(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == uid
            && metadata.mode() & 0o022 == 0,
        "invalid regular object custody",
    )?;
    Ok(file)
}
fn current_sealed_utility() -> io::Result<File> {
    let actual = File::open("/proc/self/exe")?;
    let metadata = actual.metadata()?;
    require(
        metadata.is_file()
            && metadata.uid() == 0
            && metadata.gid() == 0
            && metadata.nlink() == 1
            && metadata.mode() & 0o7777 == 0o755,
        "current utility is not root sealed",
    )?;
    let path = std::env::current_exe()?;
    let parent = path
        .parent()
        .ok_or_else(|| rejected("utility parent unavailable"))?;
    require(
        path.file_name().and_then(|s| s.to_str()) == Some("helper")
            && parent.parent() == Some(Path::new("/tmp"))
            && parent
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("polaris-helper-bootstrap.")),
        "current utility outside sealed bootstrap namespace",
    )?;
    let held_parent = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(parent)?;
    let private = held_parent.metadata()?;
    require(
        private.uid() == 0 && private.gid() == 0 && private.mode() & 0o7777 == 0o700,
        "utility bootstrap parent is not private",
    )?;
    no_extended_acl(&held_parent)?;
    let named = open_regular(&path, 0)?;
    require(
        image_objects_match(&actual, &named)?
            && Object::of(&held_parent.metadata()?) == Object::of(&fs::symlink_metadata(parent)?),
        "utility bootstrap binding changed",
    )?;
    Ok(actual)
}

// Each terminal and ancestor is held. FD9 is *only* the inherited lease;
// the actual directory descriptor below is independently allocated by openat.
struct Directory {
    path: PathBuf,
    file: File,
    parents: Vec<(PathBuf, File)>,
}
impl Directory {
    fn open(path: &Path, legacy_terminal: bool) -> io::Result<Self> {
        use nix::fcntl::{openat, OFlag};
        use nix::sys::stat::Mode;
        require(path.is_absolute(), "absolute directory required")?;
        let mut current = File::open("/")?;
        check_directory(&current, false)?;
        let mut actual = PathBuf::from("/");
        let mut parents = Vec::new();
        let components: Vec<_> = path.components().skip(1).collect();
        require(
            !components.is_empty() && components.len() <= 32,
            "invalid directory depth",
        )?;
        for (index, component) in components.iter().enumerate() {
            let std::path::Component::Normal(name) = component else {
                return Err(rejected("directory alias"));
            };
            let next = File::from(openat(
                &current,
                Path::new(name),
                OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                Mode::empty(),
            )?);
            check_directory(&next, legacy_terminal && index + 1 == components.len())?;
            parents.push((actual.clone(), current));
            actual.push(name);
            current = next;
        }
        Ok(Self {
            path: path.to_owned(),
            file: current,
            parents,
        })
    }
    fn unchanged(&self, legacy_terminal: bool) -> io::Result<()> {
        for (path, held) in &self.parents {
            check_directory(held, false)?;
            let current = ProtectedPath::open(path, true)?;
            require(
                Object::of(&current.file.metadata()?) == Object::of(&held.metadata()?),
                "ancestor replaced",
            )?;
        }
        let current = Self::open(&self.path, legacy_terminal)?;
        require(
            Object::of(&current.file.metadata()?) == Object::of(&self.file.metadata()?),
            "directory replaced",
        )
    }
    fn seal(&self) -> io::Result<()> {
        seal_held_directory(&self.file, 0, 0, || self.unchanged(true))?;
        self.unchanged(false)
    }
}
fn check_directory(file: &File, legacy: bool) -> io::Result<()> {
    check_directory_owner(file, legacy, 0, 0)
}
fn check_directory_owner(file: &File, legacy: bool, uid: u32, gid: u32) -> io::Result<()> {
    let value = file.metadata()?;
    require(
        value.is_dir() && value.uid() == uid && value.gid() == gid,
        "directory ownership/type",
    )?;
    let mode = value.mode() & 0o7777;
    require(
        if legacy {
            matches!(mode, 0o755 | 0o775)
        } else {
            mode & 0o7022 == 0
        },
        "directory mode",
    )?;
    no_extended_acl(file)
}
fn seal_held_directory(
    file: &File,
    uid: u32,
    gid: u32,
    mut name_binding: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    name_binding()?;
    check_directory_owner(file, true, uid, gid)?;
    nix::sys::stat::fchmod(file, nix::sys::stat::Mode::from_bits_truncate(0o755))?;
    file.sync_all()?;
    check_directory_owner(file, false, uid, gid)?;
    require(
        file.metadata()?.mode() & 0o7777 == 0o755,
        "directory seal differs",
    )?;
    name_binding()
}
fn acl_failure(error: io::Error) -> io::Result<Option<Vec<u8>>> {
    if error.raw_os_error() == Some(nix::libc::ENODATA) {
        Ok(None)
    } else {
        Err(error)
    }
}
#[allow(
    unsafe_code,
    reason = "bounded Linux fgetxattr on an owned held FD, without ACL mutation"
)]
fn acl(file: &File, name: &std::ffi::CStr) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = [0u8; 4096];
    // SAFETY: the File owns a live descriptor; both pointers and the bounded
    // output slice remain valid throughout the read-only native call.
    let count = unsafe {
        nix::libc::fgetxattr(
            file.as_raw_fd(),
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if count < 0 {
        let error = io::Error::last_os_error();
        // Unsupported, denied, ERANGE and every other failure are Unknown.
        return acl_failure(error);
    }
    Ok(Some(bytes[..count as usize].to_vec()))
}
fn ordinary_access_acl(bytes: &[u8]) -> bool {
    bytes.len() == 28
        && bytes[..4] == 2u32.to_le_bytes()
        && bytes[4..]
            .chunks_exact(8)
            .zip([1u16, 4, 32])
            .all(|(entry, tag)| {
                entry[..2] == tag.to_le_bytes()
                    && u16::from_le_bytes([entry[2], entry[3]]) <= 7
                    && entry[4..8] == u32::MAX.to_le_bytes()
            })
}
fn no_extended_acl(file: &File) -> io::Result<()> {
    require(
        acl(file, c"system.posix_acl_default")?.is_none(),
        "default ACL is unsupported",
    )?;
    if let Some(value) = acl(file, c"system.posix_acl_access")? {
        require(ordinary_access_acl(&value), "extended ACL is unsupported")?;
    }
    Ok(())
}

/// Finite administrator children: inherited input is closed, both outputs are
/// drained concurrently with caps, and timeout kills/reaps the owned group.
fn utility(program: &str, args: &[&str]) -> io::Result<String> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let pid = nix::unistd::Pid::from_raw(child.id() as i32);
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| rejected("stdout custody unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| rejected("stderr custody unavailable"))?;
    let capture = |reader: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            reader.take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes)?;
            require(
                bytes.len() as u64 <= OUTPUT_LIMIT,
                "utility output exceeded bound",
            )?;
            Ok::<_, io::Error>(bytes)
        })
    };
    let output = capture(Box::new(stdout));
    let error = capture(Box::new(stderr));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut custody = true;
    let status = loop {
        if Instant::now() >= deadline {
            break Err(rejected("administrator utility timed out"));
        }
        use nix::sys::wait::{waitid, Id, WaitPidFlag, WaitStatus};
        // Keep an exited leader unreaped while either pipe is live. Its PID
        // cannot be reused as another owner's process-group ID on timeout.
        let exited = match waitid(
            Id::Pid(pid),
            WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT | WaitPidFlag::WNOHANG,
        ) {
            Ok(WaitStatus::StillAlive) => false,
            Ok(_) => true,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(error) => {
                if error == nix::errno::Errno::ECHILD {
                    custody = false;
                }
                break Err(io::Error::from(error));
            }
        };
        if exited && output.is_finished() && error.is_finished() {
            break child.wait();
        }
        if Instant::now() >= deadline {
            break Err(rejected("administrator utility timed out"));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if status.is_err() {
        if custody {
            let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
            let _ = child.kill();
            let _ = child.wait();
        }
        let drain_deadline = Instant::now() + Duration::from_secs(1);
        while !(output.is_finished() && error.is_finished()) && Instant::now() < drain_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        require(
            output.is_finished() && error.is_finished(),
            "utility pipe closure remains unknown",
        )?;
    }
    let bytes = output
        .join()
        .map_err(|_| rejected("stdout worker failed"))??;
    let _stderr = error
        .join()
        .map_err(|_| rejected("stderr worker failed"))??;
    require(status?.success(), "administrator utility failed")?;
    String::from_utf8(bytes).map_err(|_| rejected("non-UTF8 utility output"))
}
fn systemctl(args: &[&str]) -> io::Result<String> {
    utility("/usr/bin/systemctl", args)
}
fn property(key: &str) -> io::Result<String> {
    let argument = format!("--property={key}");
    let raw = systemctl(&["--system", "show", "--all", &argument, SERVICE])?;
    let line = raw.strip_suffix('\n').unwrap_or(&raw);
    require(
        !line.contains(['\n', '\r']) && line.starts_with(&format!("{key}=")),
        "missing/duplicate manager property",
    )?;
    Ok(line[key.len() + 1..].to_owned())
}
fn shared_cohort_closed() -> io::Result<()> {
    // This is the actual common source used by the two ordinary producers.
    // The ownmask validator, never a user-supplied boolean, precedes this call.
    let source = format!("{}\nSERVICE=polaris-helper.service\nDESCRIPTOR=/etc/systemd/system/polaris-helper.service\nlinux_cohort_closed\n",
        polaris_helper_client::manager::LINUX_SERVICE_CLOSURE_FUNCTIONS);
    utility("/bin/sh", &["-eu", "-c", &source])?;
    Ok(())
}
fn shared_manager_view() -> io::Result<()> {
    let source = format!("{}\nSERVICE=polaris-helper.service\nDESCRIPTOR=/etc/systemd/system/polaris-helper.service\nlinux_cgroup_view\n",
        polaris_helper_client::manager::LINUX_SERVICE_CLOSURE_FUNCTIONS);
    utility("/bin/sh", &["-eu", "-c", &source])?;
    Ok(())
}
fn shared_regular_scope() -> io::Result<()> {
    let source = format!("{}\nSERVICE=polaris-helper.service\nDESCRIPTOR=/etc/systemd/system/polaris-helper.service\nlinux_validate_service\n",
        polaris_helper_client::manager::LINUX_SERVICE_CLOSURE_FUNCTIONS);
    utility("/bin/sh", &["-eu", "-c", &source])?;
    Ok(())
}
fn namespaces() -> io::Result<String> {
    let mut observed = String::new();
    for name in ["pid", "mnt", "cgroup", "net", "user"] {
        let ours = fs::read_link(format!("/proc/self/ns/{name}"))?;
        let manager = fs::read_link(format!("/proc/1/ns/{name}"))?;
        require(ours == manager, "not the system manager namespace")?;
        observed.push_str(&format!("{name}={};", ours.display()));
    }
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    require(boot.trim().len() == 36, "boot identity unavailable")?;
    observed.push_str(boot.trim());
    Ok(observed)
}
fn allowed_start(value: &str) -> bool {
    let candidates = [
        "/usr/local/lib/polaris/helper --socket=/run/polaris/helper.sock --authfile=/var/lib/polaris/authorized-uids --coredir=/usr/local/lib/polaris/core",
        "/usr/local/lib/polaris/helper --socket=/run/polaris/helper.sock --coredir=/usr/local/lib/polaris/core",
    ];
    let Some(body) = value.strip_prefix("{ ").and_then(|s| s.strip_suffix(" }")) else {
        return false;
    };
    if body.contains(['{', '}', '\n', '\r']) {
        return false;
    }
    let mut fields = BTreeMap::new();
    for entry in body.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((key, value)) = entry.split_once('=') else {
            return false;
        };
        if ![
            "path",
            "argv[]",
            "ignore_errors",
            "start_time",
            "stop_time",
            "pid",
            "code",
            "status",
        ]
        .contains(&key)
            || fields.insert(key, value).is_some()
        {
            return false;
        }
    }
    fields.get("path") == Some(&HELPER)
        && fields.get("ignore_errors") == Some(&"no")
        && fields
            .get("argv[]")
            .is_some_and(|argv| candidates.contains(argv))
}
fn unit_bytes_allowed(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut section = "";
    let mut start = false;
    let mut seen = std::collections::BTreeSet::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if ["[Unit]", "[Service]", "[Install]"].contains(&line) {
            section = line;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        if !seen.insert((section, key)) || value.ends_with('\\') {
            return false;
        }
        let valid = match (section, key) {
            ("[Unit]", "Description" | "Documentation") => true,
            ("[Unit]", "After") => value == "network.target",
            ("[Service]", "Type") => value == "simple",
            ("[Service]", "ExecStart") => {
                start = true;
                allowed_start(&format!(
                    "{{ path={HELPER} ; argv[]={value} ; ignore_errors=no ; }}"
                ))
            }
            ("[Service]", "RuntimeDirectory") => value == "polaris",
            ("[Service]", "RuntimeDirectoryMode") => matches!(value, "0755" | "755"),
            ("[Service]", "Restart") => value == "on-failure",
            ("[Service]", "RestartSec") => matches!(value, "2" | "2s"),
            ("[Service]", "KillMode") => value == "control-group",
            ("[Service]", "SendSIGKILL") => value == "yes",
            ("[Service]", "FinalKillSignal") => matches!(value, "SIGKILL" | "9"),
            ("[Service]", "Delegate") => value == "no",
            ("[Service]", "User" | "Group") => matches!(value, "" | "root" | "0"),
            ("[Service]", "Slice") => value == "system.slice",
            ("[Install]", "WantedBy") => value == "multi-user.target",
            _ => false,
        };
        if !valid {
            return false;
        }
    }
    start
}
fn empty_hooks() -> io::Result<()> {
    for key in [
        "ExecStartPre",
        "ExecStartPost",
        "ExecReload",
        "ExecStop",
        "ExecStopPost",
        "DropInPaths",
        "Environment",
        "EnvironmentFiles",
        "RootDirectory",
        "RootImage",
        "SupplementaryGroups",
        "PAMName",
    ] {
        require(
            property(key)?.is_empty(),
            "foreign unit hook/environment/namespace",
        )?;
    }
    for key in ["User", "Group"] {
        require(
            matches!(property(key)?.as_str(), "" | "root" | "0"),
            "foreign unit account",
        )?;
    }
    for (key, expected) in [
        ("KillMode", "control-group"),
        ("SendSIGKILL", "yes"),
        ("FinalKillSignal", "9"),
        ("Delegate", "no"),
        ("Slice", "system.slice"),
        ("DynamicUser", "no"),
    ] {
        require(
            property(key)? == expected,
            "foreign effective unit configuration",
        )?;
    }
    require(
        matches!(
            property("ControlGroup")?.as_str(),
            "" | "/system.slice/polaris-helper.service"
        ),
        "foreign cgroup",
    )
}
fn process_birth(pid: u64) -> io::Result<u64> {
    require(pid > 0 && pid <= i32::MAX as u64, "invalid process ID")?;
    let raw = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let (_, rest) = raw
        .rsplit_once(')')
        .ok_or_else(|| rejected("malformed process birth"))?;
    decimal(
        rest.split_whitespace()
            .nth(19)
            .ok_or_else(|| rejected("missing process birth"))?,
    )
}
fn image_identity(pid: u64, installed: &File) -> io::Result<String> {
    let birth = process_birth(pid)?;
    // /proc/PID/exe is deliberately opened as the actual executable object;
    // readlink text, basename, samehash copies and old carrier pins do not qualify.
    let actual = File::open(format!("/proc/{pid}/exe"))?;
    let metadata = actual.metadata()?;
    require(
        metadata.uid() == 0
            && metadata.is_file()
            && image_objects_match(&actual, installed)?
            && process_birth(pid)? == birth,
        "actual running image differs from installed stop scope",
    )?;
    Ok(format!("{pid}:{birth}:{}", Object::of(&metadata).text()))
}
fn image_objects_match(actual: &File, installed: &File) -> io::Result<bool> {
    Ok(
        Object::of(&actual.metadata()?) == Object::of(&installed.metadata()?)
            && hash(actual)? == hash(installed)?,
    )
}
fn old_scope(directory: &Directory) -> io::Result<(Vec<u8>, String)> {
    namespaces()?;
    shared_regular_scope()?;
    directory.unchanged(true)?;
    Directory::open(Path::new(UNIT_PARENT), false)?;
    let unit = ProtectedPath::open(Path::new(UNIT), false)?;
    let bytes = read_held(&unit.file, 64 * 1024)?;
    require(
        unit.file.metadata()?.gid() == 0 && unit_bytes_allowed(&bytes),
        "foreign actual unit fragment",
    )?;
    no_extended_acl(&unit.file)?;
    require(
        property("LoadState")? == "loaded" && property("FragmentPath")? == UNIT,
        "old fixed regular unit is not loaded",
    )?;
    empty_hooks()?;
    require(
        allowed_start(&property("ExecStart")?),
        "effective ExecStart outside fixed legacy allowlist",
    )?;
    let pid = decimal(&property("MainPID")?)?;
    let active = property("ActiveState")?;
    let scope = if pid == 0 {
        require(
            matches!(active.as_str(), "inactive" | "failed"),
            "old active state is unknown",
        )?;
        "idle".to_owned()
    } else {
        require(active == "active", "old service is changing state")?;
        let installed = open_regular(Path::new(HELPER), 0)?;
        image_identity(pid, &installed)?
    };
    unit.unchanged()?;
    directory.unchanged(true)?;
    Ok((bytes, scope))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Install,
    Uninstall,
}
impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Uninstall => "uninstall",
        }
    }
}
fn phase_allowed(action: Action, phase: Phase) -> bool {
    match action {
        Action::Install => !matches!(phase, Phase::CompletedUninstall | Phase::RecoveryRequired),
        Action::Uninstall => matches!(
            phase,
            Phase::Prepared
                | Phase::FenceIntent
                | Phase::StoppedFenced
                | Phase::DirectorySealed
                | Phase::CompletedUninstall
        ),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Prepared,
    FenceIntent,
    StoppedFenced,
    DirectorySealed,
    CandidateStaged,
    CommittedActivationPending,
    Completed,
    CompletedUninstall,
    RecoveryRequired,
}
impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::FenceIntent => "fence-intent",
            Self::StoppedFenced => "stopped-fenced",
            Self::DirectorySealed => "directory-sealed",
            Self::CandidateStaged => "candidate-staged",
            Self::CommittedActivationPending => "committed-activation-pending",
            Self::Completed => "completed",
            Self::CompletedUninstall => "completed-uninstall",
            Self::RecoveryRequired => "recovery-required",
        }
    }
    fn parse(value: &str) -> io::Result<Self> {
        [
            Self::Prepared,
            Self::FenceIntent,
            Self::StoppedFenced,
            Self::DirectorySealed,
            Self::CandidateStaged,
            Self::CommittedActivationPending,
            Self::Completed,
            Self::CompletedUninstall,
            Self::RecoveryRequired,
        ]
        .into_iter()
        .find(|v| v.name() == value)
        .ok_or_else(|| rejected("unknown journal phase"))
    }
}
struct Request {
    action: Action,
    config: PathBuf,
    original: PathBuf,
    uid: u32,
    source: Option<PathBuf>,
    manifest: Option<PathBuf>,
    helper_sha: String,
    manifest_sha: String,
    id: String,
    token: Option<PathBuf>,
}
impl Request {
    fn parse(args: &[String]) -> io::Result<Self> {
        let action = match args.first().map(String::as_str) {
            Some("linux-legacy-install") => Action::Install,
            Some("linux-legacy-uninstall") => Action::Uninstall,
            _ => return Err(rejected("unknown legacy action")),
        };
        let expected: &[&str] = if action == Action::Install {
            &[
                "--config",
                "--original-file",
                "--source",
                "--manifest",
                "--helper-sha",
                "--manifest-sha",
                "--id",
                "--token-file",
            ]
        } else {
            &["--config", "--original-file"]
        };
        require(
            args.len() == 1 + expected.len() * 2,
            "invalid finite maintenance arguments",
        )?;
        let mut values = BTreeMap::new();
        for (pair, key) in args[1..].chunks_exact(2).zip(expected) {
            require(
                pair[0] == *key && !pair[1].contains(['\n', '\r', '\0']),
                "invalid maintenance argument",
            )?;
            values.insert(*key, pair[1].clone());
        }
        let config = PathBuf::from(&values["--config"]);
        require(
            config.is_absolute(),
            "absolute responsible-user config required",
        )?;
        let metadata = fs::symlink_metadata(&config)?;
        require(
            metadata.is_dir() && !metadata.file_type().is_symlink() && metadata.uid() != 0,
            "responsible-user config is unknown",
        )?;
        let myself = current_sealed_utility()?;
        let helper_sha = hash(&myself)?;
        if action == Action::Install {
            require(
                digest(&values["--helper-sha"])
                    && values["--helper-sha"] == helper_sha
                    && digest(&values["--manifest-sha"]),
                "selected utility pin differs",
            )?;
            CoreId::parse(&values["--id"]).map_err(rejected)?;
        }
        Ok(Self {
            action,
            config,
            uid: metadata.uid(),
            original: PathBuf::from(&values["--original-file"]),
            source: values.get("--source").map(PathBuf::from),
            manifest: values.get("--manifest").map(PathBuf::from),
            helper_sha,
            manifest_sha: values.get("--manifest-sha").cloned().unwrap_or_default(),
            id: values.get("--id").cloned().unwrap_or_default(),
            token: values.get("--token-file").map(PathBuf::from),
        })
    }
}
struct Journal {
    directory: Directory,
    fields: BTreeMap<String, String>,
    lease: Arc<ProtectedPath>,
}
const FIELDS: &[&str] = &[
    "kind",
    "action",
    "destination",
    "phase",
    "predecessor",
    "utility",
    "id",
    "manifest",
    "uid",
    "config",
    "directory",
    "unit-parent",
    "old-unit",
    "old-unit-sha",
    "mask",
    "mask-name",
    "lease",
    "namespace",
    "scope",
    "prior-tombstone",
];
impl Journal {
    fn open(path: &Path, lease: &Arc<ProtectedPath>) -> io::Result<Self> {
        lease.unchanged()?;
        let parent = path
            .parent()
            .ok_or_else(|| rejected("journal parent missing"))?;
        require(
            parent == Path::new(SUPPORT),
            "journal outside fixed namespace",
        )?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| rejected("invalid journal name"))?;
        require(
            ["helper-upgrade.", "helper-retired."].iter().any(|prefix| {
                name.strip_prefix(prefix).is_some_and(|suffix| {
                    suffix.len() == 24 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
                })
            }),
            "invalid journal name",
        )?;
        let directory = Directory::open(path, false)?;
        require(
            directory.file.metadata()?.mode() & 0o7777 == 0o700,
            "journal is not private",
        )?;
        let file = open_regular(&path.join("legacy-transaction"), 0)?;
        require(
            file.metadata()?.mode() & 0o777 == 0o600,
            "journal record is not private",
        )?;
        let fields = parse_fields(&read_held(&file, JOURNAL_LIMIT)?)?;
        require(
            fields.get("lease") == Some(&Object::of(&lease.file.metadata()?).text()),
            "lease object changed",
        )?;
        require(
            fields.get("kind").map(String::as_str) == Some(KIND)
                && fields.get("destination").map(String::as_str) == Some(HELPER),
            "journal destination/kind differs",
        )?;
        let value = Self {
            directory,
            fields,
            lease: Arc::clone(lease),
        };
        value.validate_binding()?;
        Ok(value)
    }
    fn get(&self, key: &str) -> io::Result<&str> {
        self.fields
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| rejected("missing journal field"))
    }
    fn phase(&self) -> io::Result<Phase> {
        let value = Phase::parse(self.get("phase")?)?;
        if value == Phase::RecoveryRequired {
            let previous = Phase::parse(self.get("predecessor")?)?;
            require(
                previous != Phase::RecoveryRequired,
                "unknown recovery predecessor",
            )?;
            Ok(previous)
        } else {
            Ok(value)
        }
    }
    fn validate_binding(&self) -> io::Result<()> {
        let action = match self.get("action")? {
            "install" => Action::Install,
            "uninstall" => Action::Uninstall,
            _ => return Err(rejected("unknown journal action")),
        };
        let phase = self.phase()?;
        require(phase_allowed(action, phase), "cross-action journal phase")?;
        require(
            digest(self.get("utility")?) && digest(self.get("old-unit-sha")?),
            "invalid journal digest",
        )?;
        for key in ["directory", "unit-parent", "old-unit", "lease"] {
            Object::parse(self.get(key)?)?;
        }
        if !self.get("mask")?.is_empty() {
            Object::parse(self.get("mask")?)?;
        } else {
            require(phase == Phase::Prepared, "missing mask identity")?;
        }
        let uid = decimal(self.get("uid")?)?;
        require(
            uid > 0 && uid <= u32::MAX as u64,
            "invalid responsible user",
        )?;
        let config = Path::new(self.get("config")?);
        require(
            config.is_absolute()
                && config
                    .components()
                    .skip(1)
                    .all(|p| matches!(p, std::path::Component::Normal(_))),
            "invalid journal config",
        )?;
        let suffix = self
            .directory
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split_once('.').map(|(_, v)| v))
            .ok_or_else(|| rejected("invalid journal suffix"))?;
        require(
            self.get("mask-name")? == format!(".polaris-helper.service.legacy-{suffix}"),
            "foreign mask temporary name",
        )?;
        let prior = self.get("prior-tombstone")?;
        require(
            prior.is_empty()
                || prior
                    .strip_prefix("helper-retired.")
                    .is_some_and(|s| s.len() == 24 && s.bytes().all(|b| b.is_ascii_hexdigit())),
            "foreign predecessor namespace",
        )?;
        let namespace = self.get("namespace")?;
        let parts: Vec<_> = namespace.split(';').collect();
        require(
            parts.len() == 6
                && [
                    "pid=pid:[",
                    "mnt=mnt:[",
                    "cgroup=cgroup:[",
                    "net=net:[",
                    "user=user:[",
                ]
                .iter()
                .zip(&parts)
                .all(|(prefix, part)| {
                    part.strip_prefix(prefix)
                        .and_then(|s| s.strip_suffix(']'))
                        .is_some_and(|s| decimal(s).is_ok())
                })
                && parts[5].len() == 36
                && parts[5].bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
            "invalid namespace/boot record",
        )?;
        let scope = self.get("scope")?;
        require(
            matches!(scope, "idle" | "reinstall-from-own-tombstone") || {
                let parts: Vec<_> = scope.split(':').collect();
                parts.len() == 4 && parts.iter().all(|s| decimal(s).is_ok())
            },
            "invalid process birth record",
        )?;
        if action == Action::Install {
            require(
                digest(self.get("manifest")?),
                "invalid selected manifest pin",
            )?;
            CoreId::parse(self.get("id")?).map_err(rejected)?;
        } else {
            require(
                self.get("manifest")?.is_empty() && self.get("id")?.is_empty(),
                "uninstall must not depend on core selection",
            )?;
        }
        Ok(())
    }
    fn check(&self) -> io::Result<()> {
        self.directory.unchanged(false)?;
        self.lease.unchanged()
    }
    fn save(&self) -> io::Result<()> {
        self.check()?;
        let mut bytes = Vec::new();
        for (key, value) in &self.fields {
            require(
                FIELDS.contains(&key.as_str()) && !value.contains(['\n', '\r', '\0']),
                "invalid journal field",
            )?;
            bytes.extend_from_slice(format!("{key}={value}\n").as_bytes());
        }
        atomic_data(&self.directory, "legacy-transaction", &bytes, 0o600)
    }
    fn set_phase(&mut self, phase: Phase) -> io::Result<()> {
        self.fields.insert("phase".into(), phase.name().into());
        self.fields
            .insert("predecessor".into(), phase.name().into());
        self.save()
    }
    fn recovery(&mut self) {
        if let Ok(phase) = self.phase() {
            self.fields
                .insert("predecessor".into(), phase.name().into());
            self.fields
                .insert("phase".into(), Phase::RecoveryRequired.name().into());
            let _ = self.save();
        }
    }
    fn data(&self, name: &str, limit: u64) -> io::Result<Vec<u8>> {
        self.check()?;
        let held = ProtectedPath::open(&self.directory.path.join(name), false)?;
        read_held(&held.file, limit)
    }
    fn put(&self, name: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
        self.check()?;
        atomic_data(&self.directory, name, bytes, mode)
    }
    fn ownmask(&self) -> io::Result<()> {
        self.check()?;
        let parent = Directory::open(Path::new(UNIT_PARENT), false)?;
        require(
            Object::of(&parent.file.metadata()?).text() == self.get("unit-parent")?,
            "descriptor parent replaced",
        )?;
        let metadata = fs::symlink_metadata(UNIT)?;
        require(
            metadata.file_type().is_symlink()
                && metadata.uid() == 0
                && metadata.gid() == 0
                && metadata.nlink() == 1
                && Object::of(&metadata).text() == self.get("mask")?
                && fs::read_link(UNIT)? == Path::new("/dev/null"),
            "foreign or replaced canonical mask",
        )?;
        parent.unchanged(false)
    }
    fn manager_masked(&self) -> io::Result<()> {
        self.ownmask()?;
        namespaces()?;
        shared_manager_view()?;
        require(
            property("LoadState")? == "masked" && property("FragmentPath")? == "/dev/null",
            "manager has not loaded ownmask",
        )?;
        empty_hooks()?;
        self.ownmask()
    }
    fn close_fenced(&self) -> io::Result<()> {
        self.ownmask()?;
        let current_namespace = namespaces()?;
        shared_manager_view()?;
        systemctl(&["--system", "daemon-reload"])?;
        self.manager_masked()?;
        let pid = decimal(&property("MainPID")?)?;
        if pid > 0 {
            require(
                current_namespace == self.get("namespace")?,
                "service owner is from a different boot or namespace",
            )?;
            let directory = Directory::open(Path::new(INSTALL), true)?;
            require(
                Object::of(&directory.file.metadata()?).text() == self.get("directory")?,
                "stop scope directory changed",
            )?;
            let installed = open_regular(Path::new(HELPER), 0)?;
            require(
                image_identity(pid, &installed)? == self.get("scope")?,
                "new service owner after fence intent",
            )?;
            directory.unchanged(true)?;
        }
        systemctl(&["--system", "stop", SERVICE])?;
        self.manager_masked()?;
        shared_cohort_closed()?;
        self.ownmask()
    }
}
fn parse_fields(bytes: &[u8]) -> io::Result<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).map_err(|_| rejected("journal is not UTF8"))?;
    require(
        text.ends_with('\n') && !text.contains(['\r', '\0']),
        "malformed journal",
    )?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| rejected("malformed journal field"))?;
        require(
            FIELDS.contains(&key) && values.insert(key.to_owned(), value.to_owned()).is_none(),
            "unknown/duplicate journal field",
        )?;
    }
    require(values.len() == FIELDS.len(), "incomplete finite journal")?;
    Ok(values)
}
fn atomic_data(parent: &Directory, name: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
    atomic_publication(parent, name, bytes, mode, None)
}
fn atomic_publication(
    parent: &Directory,
    name: &str,
    bytes: &[u8],
    mode: u32,
    ownmask: Option<&Journal>,
) -> io::Result<()> {
    require(
        !name.contains('/') && name != "." && name != "..",
        "invalid atomic leaf",
    )?;
    parent.unchanged(false)?;
    let previous = match fs::symlink_metadata(parent.path.join(name)) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let journal =
                ownmask.ok_or_else(|| rejected("symlink is not an ordinary publication target"))?;
            require(
                parent.path == Path::new(UNIT_PARENT)
                    && name == SERVICE
                    && journal.phase()? == Phase::CommittedActivationPending,
                "ownmask publication outside committed dispatcher",
            )?;
            journal.ownmask()?;
            Some(Object::of(&metadata))
        }
        Ok(_) => {
            require(ownmask.is_none(), "expected ownmask was replaced")?;
            let held = ProtectedPath::open(&parent.path.join(name), false)?;
            no_extended_acl(&held.file)?;
            Some(Object::of(&held.file.metadata()?))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && ownmask.is_none() => None,
        Err(error) => return Err(error),
    };
    let temporary = parent.path.join(format!(".{name}.next.{}", nonce()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(&temporary)?;
    file.write_all(bytes)?;
    // The root wrapper uses umask 077. Publication modes are explicit on this
    // held new FD, not inferred from OpenOptions' umask-filtered create mode.
    nix::sys::stat::fchmod(&file, nix::sys::stat::Mode::from_bits_truncate(mode))?;
    require(
        file.metadata()?.mode() & 0o7777 == mode,
        "atomic publication mode differs",
    )?;
    file.sync_all()?;
    require(
        Object::of(&file.metadata()?) == Object::of(&fs::symlink_metadata(&temporary)?),
        "temporary data replaced",
    )?;
    parent.unchanged(false)?;
    match fs::symlink_metadata(parent.path.join(name)) {
        Ok(metadata) => require(
            Some(Object::of(&metadata)) == previous,
            "publication target changed",
        )?,
        Err(error) if error.kind() == io::ErrorKind::NotFound && previous.is_none() => {}
        Err(error) => return Err(error),
    }
    fs::rename(&temporary, parent.path.join(name))?;
    parent.file.sync_all()?;
    parent.unchanged(false)
}
fn baseline(bytes: &[u8]) -> io::Result<[String; 10]> {
    let text = std::str::from_utf8(bytes).map_err(|_| rejected("baseline is not UTF8"))?;
    let values: Vec<_> = text.lines().map(str::to_owned).collect();
    require(
        text.ends_with('\n')
            && values.len() == 10
            && values
                .iter()
                .all(|v| !v.is_empty() && v.len() <= 4096 && !v.contains(['\r', '\0'])),
        "responsible-user baseline is unknown",
    )?;
    require(
        Path::new(&values[9]).is_absolute(),
        "responsible HOME is not absolute",
    )?;
    values
        .try_into()
        .map_err(|_| rejected("invalid baseline count"))
}
fn user_readback(
    uid: u32,
    config: &Path,
    original: &[String; 10],
    retained_authorization: Option<&[u8]>,
) -> io::Result<()> {
    require(
        fs::symlink_metadata(config)?.uid() == uid,
        "responsible config owner changed",
    )?;
    let path = Path::new(SUPPORT).join("authorized-uids");
    let authorization = match fs::symlink_metadata(&path) {
        Ok(_) => Some(ProtectedPath::open(&path, false)?),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && retained_authorization.is_some() =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    let bytes = if let Some(held) = &authorization {
        read_held(&held.file, 4096)?
    } else {
        retained_authorization
            .ok_or_else(|| rejected("responsible authorization unavailable"))?
            .to_vec()
    };
    let raw = String::from_utf8(bytes).map_err(|_| rejected("invalid authorization"))?;
    let owners: std::collections::BTreeSet<_> =
        raw.lines().map(decimal).collect::<io::Result<_>>()?;
    require(
        owners.len() == 1 && owners.contains(&u64::from(uid)),
        "responsible users are unknown or incomplete",
    )?;
    let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))?
        .ok_or_else(|| rejected("responsible user missing"))?;
    require(
        Path::new(&original[9]) == user.dir,
        "captured HOME differs from actual responsible user",
    )?;
    let runtime = PathBuf::from(format!("/run/user/{uid}"));
    let metadata = fs::symlink_metadata(&runtime)?;
    require(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == uid
            && metadata.mode() & 0o077 == 0,
        "responsible user session unavailable",
    )?;
    let bus = fs::symlink_metadata(runtime.join("bus"))?;
    require(
        bus.file_type().is_socket() && bus.uid() == uid,
        "responsible user bus unavailable",
    )?;
    let home = format!("HOME={}", user.dir.display());
    let xdg = format!("XDG_RUNTIME_DIR={}", runtime.display());
    let dbus = format!(
        "DBUS_SESSION_BUS_ADDRESS=unix:path={}/bus",
        runtime.display()
    );
    for ((schema, key), expected) in KEYS.iter().zip(original) {
        let actual = utility(
            "/usr/sbin/runuser",
            &[
                "--user",
                &user.name,
                "--",
                "/usr/bin/env",
                "--ignore-environment",
                &home,
                &xdg,
                &dbus,
                "PATH=/usr/bin:/bin",
                "LC_ALL=C",
                "/usr/bin/gsettings",
                "get",
                schema,
                key,
            ],
        )?;
        require(
            actual.strip_suffix('\n') == Some(expected.as_str()),
            "actual responsible-user settings differ from baseline",
        )?;
    }
    if let Some(held) = authorization {
        held.unchanged()?;
    }
    Ok(())
}
fn network_readback() -> io::Result<String> {
    namespaces()?;
    let links = utility("/usr/sbin/ip", &["-o", "link", "show"])?;
    let mut seen = std::collections::BTreeSet::new();
    require(!links.trim().is_empty(), "link enumeration unavailable")?;
    for line in links.lines() {
        let mut parts = line.splitn(3, ':');
        let index = decimal(parts.next().unwrap_or_default().trim())?;
        let name = parts
            .next()
            .ok_or_else(|| rejected("malformed link record"))?
            .trim();
        require(
            index > 0 && !name.is_empty() && seen.insert(index),
            "incomplete/duplicate link inventory",
        )?;
        let name = name.split('@').next().unwrap_or_default();
        require(
            name != polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME,
            "fixed Polaris TUN responsibility remains",
        )?;
        require(parts.next().is_some(), "truncated link record")?;
    }
    let mut report = links;
    for query in ["dns", "domain", "default-route"] {
        let values = utility("/usr/bin/resolvectl", &[query])?;
        require(
            !values.trim().is_empty(),
            "resolved enumeration unavailable",
        )?;
        for line in values.lines() {
            require(
                line.starts_with("Global:") || line.starts_with("Link "),
                "incomplete resolved inventory",
            )?;
            require(
                !line.contains(polaris_helper_proto::linux_dns::TUN_INTERFACE_NAME),
                "fixed resolved link responsibility remains",
            )?;
        }
        report.push_str(&format!("\n{query}\n{values}"));
    }
    Ok(report)
}
fn fresh_release(journal: &Journal) -> io::Result<String> {
    let uid = u32::try_from(decimal(journal.get("uid")?)?).map_err(|_| rejected("invalid user"))?;
    let retained = journal.data("original-authorization", 4096)?;
    let allow_retained =
        journal.get("action")? == "uninstall" || !journal.get("prior-tombstone")?.is_empty();
    user_readback(
        uid,
        Path::new(journal.get("config")?),
        &baseline(&journal.data("original-proxy", JOURNAL_LIMIT)?)?,
        allow_retained.then_some(retained.as_slice()),
    )?;
    network_readback()
}

fn classify() -> io::Result<&'static str> {
    if let Some(path) = find_journal()? {
        if fs::symlink_metadata(path.join("legacy-transaction")).is_ok() {
            // In particular, a committed new regular unit/receipt must resume
            // its existing legacy journal, not fall into ordinary rollback.
            return Ok("legacy");
        }
    }
    let parent = match fs::symlink_metadata(INSTALL) {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            require(
                !Path::new(UNIT).try_exists()?,
                "descriptor exists without installation",
            )?;
            return Ok("ordinary");
        }
        Err(e) => return Err(e),
    };
    require(
        parent.is_dir()
            && !parent.file_type().is_symlink()
            && parent.uid() == 0
            && parent.gid() == 0,
        "invalid installation terminal",
    )?;
    if parent.mode() & 0o7777 == 0o775 {
        return Ok("legacy");
    }
    require(
        parent.mode() & 0o7777 == 0o755,
        "unsupported installation terminal mode",
    )?;
    let unit_exists = match fs::symlink_metadata(UNIT) {
        Ok(value) if value.file_type().is_symlink() => return Ok("legacy"),
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if !Path::new(HELPER).try_exists()? {
        return Ok(if unit_exists { "legacy" } else { "ordinary" });
    }
    let policy = crate::helper_policy::policy(
        Path::new(HELPER),
        &Path::new(SUPPORT).join("helper-installed.json"),
    )?;
    Ok(if policy == "retain" {
        "ordinary"
    } else {
        "legacy"
    })
}

fn find_journal() -> io::Result<Option<PathBuf>> {
    let support = Directory::open(Path::new(SUPPORT), false)?;
    let mut result = None;
    for entry in fs::read_dir(&support.path)? {
        let entry = entry?;
        require(
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with("helper-admission."),
            "incomplete admission journal retained; no new transaction may bypass it",
        )?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("helper-upgrade.")
        {
            require(
                result.replace(entry.path()).is_none(),
                "multiple active or unknown journals",
            )?;
        }
    }
    support.unchanged(false)?;
    Ok(result)
}
fn open_tombstone(lease: &Arc<ProtectedPath>) -> io::Result<Journal> {
    let pointer = ProtectedPath::open(&Path::new(SUPPORT).join("helper-legacy-tombstone"), false)?;
    let bytes = read_held(&pointer.file, 256)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| rejected("invalid tombstone"))?;
    let names: Vec<_> = text.lines().collect();
    require(
        names.len() == 2
            && names[0].starts_with("helper-upgrade.")
            && names[1].starts_with("helper-retired.")
            && names[0].strip_prefix("helper-upgrade.") == names[1].strip_prefix("helper-retired."),
        "unbound tombstone names",
    )?;
    let mut result = None;
    for name in names {
        let path = Path::new(SUPPORT).join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                require(result.is_none(), "ambiguous tombstone journals")?;
                result = Some(Journal::open(&path, lease)?);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let journal = result.ok_or_else(|| rejected("tombstone journal unavailable"))?;
    require(
        journal.phase()? == Phase::CompletedUninstall && journal.get("action")? == "uninstall",
        "foreign tombstone action",
    )?;
    journal.manager_masked()?;
    pointer.unchanged()?;
    Ok(journal)
}
// A valid newer receipt is a refusal constraint, never old executable trust.
// Read its data through the legacy directory FD contract before masking; the
// ordinary helper-policy reader correctly rejects a 0775 parent and cannot be
// used until sealing. Known journals bypass this preflight and continue forward.
fn preflight_selected_policy(request: &Request) -> io::Result<()> {
    if request.action != Action::Install || !Path::new(HELPER).try_exists()? {
        return Ok(());
    }
    let directory = Directory::open(Path::new(INSTALL), true)?;
    let receipt_path = Path::new(SUPPORT).join("helper-installed.json");
    let receipt = match ProtectedPath::open(&receipt_path, false) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let record = polaris_helper_proto::core_manifest::InstalledHelperRecord::parse(&read_held(
        &receipt.file,
        4096,
    )?)
    .map_err(rejected)?;
    let installed = open_regular(Path::new(HELPER), 0)?;
    let actual = hash(&installed)?;
    require(
        actual == record.sha256,
        "installed helper receipt differs before fencing",
    )?;
    directory.unchanged(true)?;
    require(
        Object::of(&fs::symlink_metadata(HELPER)?) == Object::of(&installed.metadata()?),
        "installed helper replaced before fencing",
    )?;
    receipt.unchanged()?;
    require(
        actual == request.helper_sha
            || crate::helper_policy::may_replace(
                &record.version,
                record.protocol,
                env!("CARGO_PKG_VERSION"),
                polaris_helper_proto::proto_version::CURRENT,
            )?,
        "newer compatible helper must be retained; selected downgrade refused before fencing",
    )
}

fn prepare(request: &Request, lease: &Arc<ProtectedPath>) -> io::Result<Journal> {
    lease.unchanged()?;
    if let Some(path) = find_journal()? {
        let mut journal = Journal::open(&path, lease)?;
        if request.action == Action::Install
            && journal.get("action")? == "uninstall"
            && journal.phase()? == Phase::CompletedUninstall
        {
            journal.close_fenced()?;
            fresh_release(&journal)?;
            retire(&mut journal)?;
            return prepare(request, lease);
        }
        require(
            journal.get("action")? == request.action.name()
                && journal.get("uid")? == request.uid.to_string()
                && Path::new(journal.get("config")?) == request.config,
            "cross-action/user journal",
        )?;
        if request.action == Action::Install {
            require(
                journal.get("utility")? == request.helper_sha
                    && journal.get("id")? == request.id
                    && journal.get("manifest")? == request.manifest_sha,
                "recovery candidate differs from selected pin",
            )?;
        }
        return Ok(journal);
    }
    if request.action == Action::Uninstall && fs::symlink_metadata(UNIT)?.file_type().is_symlink() {
        let journal = open_tombstone(lease)?;
        require(
            journal.get("uid")? == request.uid.to_string()
                && Path::new(journal.get("config")?) == request.config,
            "tombstone responsible user differs",
        )?;
        journal.close_fenced()?;
        fresh_release(&journal)?;
        return Ok(journal);
    }
    preflight_selected_policy(request)?;
    let directory = Directory::open(Path::new(INSTALL), true)?;
    let unit_parent = Directory::open(Path::new(UNIT_PARENT), false)?;
    let mut prior = String::new();
    let (original_unit, scope, old_unit, old_sha, mask) =
        if fs::symlink_metadata(UNIT)?.file_type().is_symlink() {
            require(
                request.action == Action::Install,
                "completed uninstall must not be repeated as fresh removal",
            )?;
            let predecessor = open_tombstone(lease)?;
            require(
                Object::of(&directory.file.metadata()?).text() == predecessor.get("directory")?,
                "tombstone installation directory was replaced",
            )?;
            predecessor.manager_masked()?;
            predecessor.close_fenced()?;
            fresh_release(&predecessor)?;
            prior = predecessor
                .directory
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| rejected("invalid predecessor"))?
                .to_owned();
            (
                predecessor.data("old-unit-data", 64 * 1024)?,
                "reinstall-from-own-tombstone".into(),
                predecessor.get("old-unit")?.to_owned(),
                predecessor.get("old-unit-sha")?.to_owned(),
                predecessor.get("mask")?.to_owned(),
            )
        } else {
            let (bytes, scope) = old_scope(&directory)?;
            let metadata = fs::symlink_metadata(UNIT)?;
            let sha = crate::core_hash::sha256_hex(&bytes);
            (
                bytes,
                scope,
                Object::of(&metadata).text(),
                sha,
                String::new(),
            )
        };
    let release_file = open_regular(&request.original, 0)?;
    let mut original = read_held(&release_file, JOURNAL_LIMIT)?;
    if original.is_empty() && !prior.is_empty() {
        original = Journal::open(&Path::new(SUPPORT).join(&prior), lease)?
            .data("original-proxy", JOURNAL_LIMIT)?;
    }
    let original_values = baseline(&original)?;
    let authorization = if prior.is_empty() {
        let held = ProtectedPath::open(&Path::new(SUPPORT).join("authorized-uids"), false)?;
        read_held(&held.file, 4096)?
    } else {
        Journal::open(&Path::new(SUPPORT).join(&prior), lease)?
            .data("original-authorization", 4096)?
    };
    user_readback(
        request.uid,
        &request.config,
        &original_values,
        (!prior.is_empty()).then_some(authorization.as_slice()),
    )?;
    network_readback()?;
    let support = Directory::open(Path::new(SUPPORT), false)?;
    lease.unchanged()?;
    let suffix = nonce()?;
    let admission = support.path.join(format!("helper-admission.{suffix}"));
    fs::DirBuilder::new().mode(0o700).create(&admission)?;
    let mut fields: BTreeMap<String, String> = FIELDS
        .iter()
        .map(|s| ((*s).to_owned(), String::new()))
        .collect();
    for (key, value) in [
        ("kind", KIND.to_owned()),
        ("action", request.action.name().to_owned()),
        ("destination", HELPER.to_owned()),
        ("phase", Phase::Prepared.name().into()),
        ("predecessor", Phase::Prepared.name().into()),
        ("utility", request.helper_sha.clone()),
        ("id", request.id.clone()),
        ("manifest", request.manifest_sha.clone()),
        ("uid", request.uid.to_string()),
        ("config", request.config.to_string_lossy().into_owned()),
        ("directory", Object::of(&directory.file.metadata()?).text()),
        (
            "unit-parent",
            Object::of(&unit_parent.file.metadata()?).text(),
        ),
        ("old-unit", old_unit),
        ("old-unit-sha", old_sha),
        ("mask", mask),
        (
            "mask-name",
            format!(".polaris-helper.service.legacy-{suffix}"),
        ),
        ("lease", Object::of(&lease.file.metadata()?).text()),
        ("namespace", namespaces()?),
        ("scope", scope),
        ("prior-tombstone", prior),
    ] {
        fields.insert(key.into(), value);
    }
    let mut journal = Journal {
        directory: Directory::open(&admission, false)?,
        fields,
        lease: Arc::clone(lease),
    };
    journal.put("old-unit-data", &original_unit, 0o600)?;
    journal.put("original-proxy", &original, 0o600)?;
    journal.put("original-authorization", &authorization, 0o600)?;
    if Path::new(HELPER).try_exists()? {
        let old = open_regular(Path::new(HELPER), 0)?;
        journal.put("old-helper-data", &read_held(&old, IMAGE_LIMIT)?, 0o600)?;
    }
    journal.save()?;
    journal.directory.file.sync_all()?;
    let published = support.path.join(format!("helper-upgrade.{suffix}"));
    fs::rename(&admission, &published)?;
    support.file.sync_all()?;
    journal.directory = Directory::open(&published, false)?;
    Ok(journal)
}

fn fence(journal: &mut Journal) -> io::Result<()> {
    let parent = Directory::open(Path::new(UNIT_PARENT), false)?;
    require(
        Object::of(&parent.file.metadata()?).text() == journal.get("unit-parent")?,
        "descriptor parent changed",
    )?;
    let canonical = fs::symlink_metadata(UNIT)?;
    if canonical.file_type().is_symlink() {
        journal.ownmask()?;
    } else {
        let directory = Directory::open(Path::new(INSTALL), true)?;
        require(
            Object::of(&directory.file.metadata()?).text() == journal.get("directory")?,
            "installation directory changed",
        )?;
        let (bytes, fresh_scope) = old_scope(&directory)?;
        require(
            Object::of(&canonical).text() == journal.get("old-unit")?
                && crate::core_hash::sha256_hex(&bytes) == journal.get("old-unit-sha")?,
            "original unit changed",
        )?;
        fresh_release(journal)?;
        let (rechecked_bytes, rechecked_scope) = old_scope(&directory)?;
        require(
            rechecked_scope == fresh_scope && rechecked_bytes == bytes,
            "old owner changed during responsible network readback",
        )?;
        // A new invocation/boot must observe and bind the current actual scope;
        // historical PID/birth/namespace strings never authorize a signal.
        journal.fields.insert("scope".into(), fresh_scope);
        journal.fields.insert("namespace".into(), namespaces()?);
        journal.save()?;
        let temporary = parent.path.join(journal.get("mask-name")?);
        if journal.get("mask")?.is_empty() {
            // Save the actual fragment data and a persistent actual mask/parent
            // binding *before* replacing the canonical descriptor. Cross-FS
            // fragment preservation is copied data, never called an inode move.
            require(
                journal.data("old-unit-data", 64 * 1024)? == bytes,
                "original fragment evidence differs",
            )?;
            std::os::unix::fs::symlink("/dev/null", &temporary)?;
            parent.file.sync_all()?;
            let metadata = fs::symlink_metadata(&temporary)?;
            journal
                .fields
                .insert("mask".into(), Object::of(&metadata).text());
            journal.set_phase(Phase::FenceIntent)?;
        }
        let metadata = fs::symlink_metadata(&temporary)?;
        require(
            metadata.file_type().is_symlink()
                && metadata.uid() == 0
                && metadata.nlink() == 1
                && Object::of(&metadata).text() == journal.get("mask")?
                && fs::read_link(&temporary)? == Path::new("/dev/null"),
            "intent mask object changed",
        )?;
        require(
            Object::of(&fs::symlink_metadata(UNIT)?).text() == journal.get("old-unit")?,
            "canonical old fragment replaced",
        )?;
        parent.unchanged(false)?;
        fs::rename(&temporary, UNIT)?;
        parent.file.sync_all()?;
    }
    // An intent whose mask rename reached disk but whose phase did not is
    // recognized by its persisted actual mask binding, never by target text.
    journal.close_fenced()?;
    fresh_release(journal)?;
    journal.set_phase(Phase::StoppedFenced)
}
fn preserve_flat(journal: &Journal) -> io::Result<()> {
    for name in ["sing-box", "sing-box.exe", "libcronet.so", "libcronet.dll"] {
        let path = Path::new(CORE).join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let held = ProtectedPath::open(&path, false)?;
                let target = format!("old-flat-{name}-data");
                let bytes = read_held(&held.file, IMAGE_LIMIT)?;
                if journal.directory.path.join(&target).try_exists()? {
                    require(
                        journal.data(&target, IMAGE_LIMIT)? == bytes,
                        "flat evidence changed",
                    )?;
                } else {
                    journal.put(&target, &bytes, 0o600)?;
                }
                held.unchanged()?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
fn stage(journal: &mut Journal, request: &Request) -> io::Result<()> {
    journal.manager_masked()?;
    journal.close_fenced()?;
    fresh_release(journal)?;
    let directory = Directory::open(Path::new(INSTALL), true)?;
    require(
        Object::of(&directory.file.metadata()?).text() == journal.get("directory")?,
        "seal directory changed",
    )?;
    directory.seal()?;
    journal.set_phase(Phase::DirectorySealed)?;
    if request.action == Action::Uninstall {
        return Ok(());
    }
    preserve_flat(journal)?;
    let source = request
        .source
        .as_ref()
        .ok_or_else(|| rejected("source missing"))?;
    let manifest_path = request
        .manifest
        .as_ref()
        .ok_or_else(|| rejected("manifest missing"))?;
    let manifest = open_regular(manifest_path, request.uid)?;
    let bytes = read_held(&manifest, 64 * 1024)?;
    require(
        crate::core_hash::sha256_hex(&bytes) == request.manifest_sha,
        "selected manifest changed",
    )?;
    let selected =
        InstalledCoreManifest::parse(&bytes, crate::core_hash::sha256_hex).map_err(rejected)?;
    require(
        selected.id == request.id
            && selected.core.platform == "linux"
            && selected.core.helper_sha256.as_deref() == Some(request.helper_sha.as_str()),
        "selected manifest/pin differs",
    )?;
    if crate::helper_policy::policy(
        Path::new(HELPER),
        &Path::new(SUPPORT).join("helper-installed.json"),
    )? == "retain"
        && hash(&open_regular(Path::new(HELPER), 0)?)? != request.helper_sha
    {
        // Never downgrade a newer compatible receipt because legacy repair was
        // explicitly requested. A fenced recovery is preferable to replacing it.
        return Err(rejected(
            "compatible retained helper requires ordinary repair; downgrade refused",
        ));
    }
    let core = Path::new(CORE);
    if !core.try_exists()? {
        fs::create_dir(core)?;
        fs::set_permissions(core, fs::Permissions::from_mode(0o755))?;
        directory.file.sync_all()?;
    }
    Directory::open(core, false)?;
    journal.put("core-manifest.json", &bytes, 0o600)?;
    crate::core_cli::install(&crate::core_cli::InstallArgs {
        root: core.to_owned(),
        source: source.clone(),
        manifest: journal.directory.path.join("core-manifest.json"),
        source_owner: Some(polaris_helper_proto::core_protocol::CallerIdentity::Unix(
            request.uid,
        )),
        preserve_existing: true,
    })?;
    let utility_image = File::open("/proc/self/exe")?;
    require(
        hash(&utility_image)? == request.helper_sha,
        "current sealed utility changed",
    )?;
    journal.put(
        "candidate-helper",
        &read_held(&utility_image, IMAGE_LIMIT)?,
        0o755,
    )?;
    journal.put(
        "candidate-unit",
        polaris_helper_client::manager::linux_canonical_unit().as_bytes(),
        0o644,
    )?;
    crate::helper_policy::record(
        &journal.directory.path.join("candidate-helper"),
        &journal.directory.path.join("candidate-receipt"),
    )?;
    let token = open_regular(
        request
            .token
            .as_ref()
            .ok_or_else(|| rejected("token missing"))?,
        0,
    )?;
    let token = read_held(&token, 128)?;
    require(
        std::str::from_utf8(&token).is_ok_and(|s| digest(s.trim())),
        "invalid registration token",
    )?;
    journal.put("user-token", &token, 0o600)?;
    crate::user_control::register(
        Path::new(SUPPORT),
        &polaris_helper_proto::core_protocol::CallerIdentity::Unix(request.uid),
        &request.config,
        &journal.directory.path.join("user-token"),
    )?;
    journal.put(
        "candidate-authorization",
        &journal.data("original-authorization", 4096)?,
        0o600,
    )?;
    let id = CoreId::parse(&request.id).map_err(rejected)?;
    let store =
        crate::core_store::CoreStore::open(core, crate::core_store::OwnerPolicy::privileged())?;
    let _verified = store.verify_for_start(&id)?;
    journal.directory.file.sync_all()?;
    journal.manager_masked()?;
    shared_cohort_closed()?;
    fresh_release(journal)?;
    journal.set_phase(Phase::CandidateStaged)
}
fn candidate_valid(journal: &Journal) -> io::Result<()> {
    let directory = Directory::open(Path::new(INSTALL), false)?;
    require(
        Object::of(&directory.file.metadata()?).text() == journal.get("directory")?,
        "candidate installation directory changed",
    )?;
    let held = ProtectedPath::open(&journal.directory.path.join("candidate-helper"), false)?;
    require(
        hash(&held.file)? == journal.get("utility")?,
        "candidate helper pin changed",
    )?;
    require(
        journal.data("candidate-unit", 64 * 1024)?
            == polaris_helper_client::manager::linux_canonical_unit().as_bytes(),
        "candidate unit differs",
    )?;
    let receipt = polaris_helper_proto::core_manifest::InstalledHelperRecord::parse(
        &journal.data("candidate-receipt", 4096)?,
    )
    .map_err(rejected)?;
    require(
        receipt.sha256 == journal.get("utility")?,
        "candidate receipt differs",
    )?;
    let id = CoreId::parse(journal.get("id")?).map_err(rejected)?;
    let store = crate::core_store::CoreStore::open(
        Path::new(CORE),
        crate::core_store::OwnerPolicy::privileged(),
    )?;
    let _verified = store.verify_for_start(&id)?;
    held.unchanged()?;
    directory.unchanged(false)
}
fn publish(journal: &Journal, source: &str, parent: &str, leaf: &str, mode: u32) -> io::Result<()> {
    let parent = Directory::open(Path::new(parent), false)?;
    if parent.path == Path::new(INSTALL) {
        require(
            Object::of(&parent.file.metadata()?).text() == journal.get("directory")?,
            "publish installation directory changed",
        )?;
    }
    let bytes = journal.data(source, IMAGE_LIMIT)?;
    if parent.path == Path::new(UNIT_PARENT) && leaf == SERVICE {
        atomic_publication(&parent, leaf, &bytes, mode, Some(journal))
    } else {
        atomic_data(&parent, leaf, &bytes, mode)
    }
}
fn activate(journal: &mut Journal) -> io::Result<()> {
    candidate_valid(journal)?;
    require(
        matches!(
            journal.phase()?,
            Phase::CandidateStaged | Phase::CommittedActivationPending | Phase::Completed
        ),
        "activation before staged candidate",
    )?;
    if journal.phase()? == Phase::CandidateStaged {
        journal.close_fenced()?;
        fresh_release(journal)?;
        // The activation fence is the persistent canonical ownmask, not FD9.
        // All candidate bytes and journal parents have been synced above.
        journal.set_phase(Phase::CommittedActivationPending)?;
    }
    let canonical = fs::symlink_metadata(UNIT)?;
    if canonical.file_type().is_symlink() {
        journal.ownmask()?;
        journal.close_fenced()?;
        fresh_release(journal)?;
        publish(journal, "candidate-helper", INSTALL, "helper", 0o755)?;
        publish(
            journal,
            "candidate-receipt",
            SUPPORT,
            "helper-installed.json",
            0o644,
        )?;
        publish(
            journal,
            "candidate-authorization",
            SUPPORT,
            "authorized-uids",
            0o600,
        )?;
        // COMMIT is already durable before exposing the ordinary unit. Never
        // move this publication into stage(), even when start is deferred.
        journal.ownmask()?;
        publish(
            journal,
            "candidate-unit",
            UNIT_PARENT,
            "polaris-helper.service",
            0o644,
        )?;
    } else {
        let held = ProtectedPath::open(Path::new(UNIT), false)?;
        require(
            read_held(&held.file, 64 * 1024)? == journal.data("candidate-unit", 64 * 1024)?,
            "postcommit descriptor has a new owner",
        )?;
        let installed = ProtectedPath::open(Path::new(HELPER), false)?;
        require(
            hash(&installed.file)? == journal.get("utility")?,
            "postcommit helper has a new owner",
        )?;
    }
    systemctl(&["--system", "daemon-reload"])?;
    let actual_unit = ProtectedPath::open(Path::new(UNIT), false)?;
    require(
        read_held(&actual_unit.file, 64 * 1024)? == journal.data("candidate-unit", 64 * 1024)?,
        "new canonical descriptor changed before activation",
    )?;
    require(
        property("LoadState")? == "loaded" && property("FragmentPath")? == UNIT,
        "new unit not loaded",
    )?;
    empty_hooks()?;
    require(
        allowed_start(&property("ExecStart")?),
        "new effective ExecStart differs",
    )?;
    systemctl(&["--system", "enable", SERVICE])?;
    // start is idempotent for this exact new image; no late restart/stop may
    // terminate a new owner after commit.
    systemctl(&["--system", "start", SERVICE])?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if property("ActiveState")? == "active" && decimal(&property("MainPID")?)? > 0 {
            break;
        }
        require(Instant::now() < deadline, "new helper activation pending")?;
        std::thread::sleep(Duration::from_millis(100));
    }
    let installed = ProtectedPath::open(Path::new(HELPER), false)?;
    require(
        hash(&installed.file)? == journal.get("utility")?,
        "new installed pin differs",
    )?;
    let pid = decimal(&property("MainPID")?)?;
    let image = image_identity(pid, &installed.file)?;
    let policy = crate::helper_policy::policy(
        Path::new(HELPER),
        &Path::new(SUPPORT).join("helper-installed.json"),
    )?;
    require(policy == "retain", "new installed receipt is not exact")?;
    let id = CoreId::parse(journal.get("id")?).map_err(rejected)?;
    crate::admin_probe::retained_ready(Path::new(SUPPORT), &id)?;
    require(
        image_identity(pid, &installed.file)? == image && decimal(&property("MainPID")?)? == pid,
        "new process changed during Ready/CoreStatus",
    )?;
    fresh_release(journal)?;
    actual_unit.unchanged()?;
    require(
        read_held(&actual_unit.file, 64 * 1024)? == journal.data("candidate-unit", 64 * 1024)?,
        "new canonical descriptor changed during Ready/CoreStatus",
    )?;
    journal.set_phase(Phase::Completed)
}
fn remove_business_tree(path: &Path, depth: usize, remaining: &mut usize) -> io::Result<()> {
    require(depth <= 24 && *remaining > 0, "removal inventory limit")?;
    *remaining -= 1;
    let metadata = fs::symlink_metadata(path)?;
    require(
        metadata.uid() == 0 && metadata.mode() & 0o022 == 0 && !metadata.file_type().is_symlink(),
        "unknown business object retained",
    )?;
    if metadata.is_dir() {
        let held = Directory::open(path, false)?;
        for entry in fs::read_dir(path)? {
            remove_business_tree(&entry?.path(), depth + 1, remaining)?;
        }
        held.unchanged(false)?;
        fs::remove_dir(path)
    } else {
        let held = ProtectedPath::open(path, false)?;
        held.unchanged()?;
        fs::remove_file(path)
    }
}
fn uninstall(journal: &mut Journal) -> io::Result<()> {
    journal.close_fenced()?;
    fresh_release(journal)?;
    let directory = Directory::open(Path::new(INSTALL), true)?;
    require(
        Object::of(&directory.file.metadata()?).text() == journal.get("directory")?,
        "uninstall directory changed",
    )?;
    directory.seal()?;
    journal.set_phase(Phase::DirectorySealed)?;
    preserve_flat(journal)?;
    let mut remaining = 32768;
    for name in ["helper", "core"] {
        let path = Path::new(INSTALL).join(name);
        if path.try_exists()? {
            remove_business_tree(&path, 0, &mut remaining)?;
        }
    }
    directory.file.sync_all()?;
    // Keep the strict empty install parent, stable lease, active/unknown
    // journals and retired data. A completed ownmask is a durable tombstone;
    // removing it could expose a vendor/default unit on the next reload/boot.
    let support = Directory::open(Path::new(SUPPORT), false)?;
    for name in [
        "helper-installed.json",
        "authorized-uids",
        "helper.token",
        "users",
    ] {
        let path = support.path.join(name);
        if path.try_exists()? {
            remove_business_tree(&path, 0, &mut remaining)?;
        }
    }
    support.file.sync_all()?;
    journal.manager_masked()?;
    shared_cohort_closed()?;
    fresh_release(journal)?;
    journal.set_phase(Phase::CompletedUninstall)
}
fn retire(journal: &mut Journal) -> io::Result<()> {
    let support = Directory::open(Path::new(SUPPORT), false)?;
    let name = journal
        .directory
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| rejected("invalid retirement name"))?;
    let suffix = name
        .strip_prefix("helper-upgrade.")
        .ok_or_else(|| rejected("not active retirement"))?;
    let retired = format!("helper-retired.{suffix}");
    require(
        !support.path.join(&retired).try_exists()?,
        "retirement destination exists",
    )?;
    journal.check()?;
    if journal.phase()? == Phase::CompletedUninstall {
        journal.ownmask()?;
        // Persist both exact names before the rename. A crash on either side
        // still has exactly one bound completed journal; no namespace search
        // or arbitrary retired directory can confer ownership of a mask.
        atomic_data(
            &support,
            "helper-legacy-tombstone",
            format!("{name}\n{retired}\n").as_bytes(),
            0o600,
        )?;
    }
    fs::rename(&journal.directory.path, support.path.join(&retired))?;
    support.file.sync_all()?;
    journal.directory = Directory::open(&support.path.join(&retired), false)?;
    if journal.phase()? == Phase::CompletedUninstall {
        journal.ownmask()?;
    }
    Ok(())
}
// FD9 is inherited from the wrapper. Opening it was not an admission lock.
// Acquire that same open-file-description before any shared decision, then keep
// this one ProtectedPath binding across current/retired journals and preserving
// Core installation. No nested journal operation reacquires or releases it.
fn acquire_admission_lease() -> io::Result<Arc<ProtectedPath>> {
    let lease = ProtectedPath::open(&Path::new(SUPPORT).join("helper-install.lease"), false)?;
    crate::process_lease::apply(&lease.path, 9, false)?;
    lease.unchanged()?;
    Ok(Arc::new(lease))
}

fn maintain(request: Request) -> io::Result<()> {
    let lease = acquire_admission_lease()?;
    let mut journal = prepare(&request, &lease)?;
    let result = (|| {
        journal.check()?;
        let phase = journal.phase()?;
        match phase {
            Phase::Prepared | Phase::FenceIntent => fence(&mut journal)?,
            Phase::StoppedFenced | Phase::DirectorySealed | Phase::CandidateStaged => {
                journal.close_fenced()?;
                fresh_release(&journal)?;
            }
            Phase::CommittedActivationPending | Phase::Completed | Phase::CompletedUninstall => {}
            Phase::RecoveryRequired => return Err(rejected("unknown recovery predecessor")),
        }
        if matches!(
            journal.phase()?,
            Phase::StoppedFenced | Phase::DirectorySealed
        ) {
            stage(&mut journal, &request)?;
        }
        match request.action {
            Action::Install => activate(&mut journal)?,
            Action::Uninstall => {
                if journal.phase()? != Phase::CompletedUninstall {
                    uninstall(&mut journal)?;
                }
                journal.manager_masked()?;
                shared_cohort_closed()?;
                network_readback()?;
            }
        }
        if journal
            .directory
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|name| name.starts_with("helper-retired."))
        {
            Ok(())
        } else {
            retire(&mut journal)
        }
    })();
    if result.is_err() {
        journal.recovery();
    }
    result
}

pub(crate) fn run(args: &[String]) -> Option<std::process::ExitCode> {
    let verb = args.first()?;
    if !verb.starts_with("linux-legacy-") {
        return None;
    }
    let result = (|| {
        require(
            nix::unistd::Uid::effective().is_root(),
            "administrator required",
        )?;
        let _utility = current_sealed_utility()?;
        if verb == "linux-legacy-classify" {
            require(
                args.len() == 3
                    && args[1] == "--operation"
                    && matches!(args[2].as_str(), "install" | "uninstall"),
                "invalid classifier arguments",
            )?;
            println!("{}", classify()?);
        } else {
            let request = Request::parse(args)?;
            let action = request.action;
            maintain(request)?;
            println!(
                "{}",
                if action == Action::Install {
                    "polaris-helper-install-ok"
                } else {
                    "polaris-helper-uninstall-state=stopped-removed-ownmask-preserved\npolaris-helper-uninstall-ok"
                }
            );
        }
        Ok::<_, io::Error>(())
    })();
    Some(match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("LINUX_LEGACY_RECOVERY_REQUIRED: {error}");
            std::process::ExitCode::FAILURE
        }
    })
}

#[cfg(test)]
mod tests;
