//! Strict, read-only facts for the explicitly requested legacy migration.
//!
//! The plist schema is CLOSED: only Label, Program, ProgramArguments, boolean
//! KeepAlive/RunAtLoad and integer ExitTimeOut are accepted. Even false values of
//! unsupported keys are rejected. In particular no credential, environment,
//! proxy, debugger, globbing, chroot or process-group configuration is ignored.
//! Serde deserializes the original plist directly, preserving duplicate-key
//! rejection; a Value/Dictionary intermediate would silently collapse duplicates.
//!
//! `launchctl print` is a diagnostic format, not a stable API. The supported
//! shape is deliberately closed and tab-indented; a new shape is an error, not
//! evidence of absence. Callers must supply complete, bounded stdout from a
//! successful command, reject stderr/truncation, and independently bind its PID
//! to the native process birth. These functions never classify command failure
//! as an absent job and perform no service operation.
//!
//! The caller keeps the existing ProtectedPath objects for plist, helper,
//! support, core directory and configured executable alive through stop proof.
//! Returned paths are lexical configuration facts, NOT new filesystem trust.
//! An optional legacy confdir is user data and is not asserted root-protected.
//!
//! Sources: Apple launchd/man/launchd.plist.5 (Program/ProgramArguments,
//! ExitTimeOut and identity-affecting keys); rust-plist v1.10.0 src/de.rs
//! (direct serde deserialization and end-of-event-stream check). No documented
//! default timeout is substituted for the actual loaded job's observed timeout.

use polaris_helper_proto::core_manifest::CoreId;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

const MAX_PLIST: usize = 64 * 1024;
const MAX_PRINT: usize = 128 * 1024;

pub struct Expected<'a> {
    pub label: &'a str,
    pub plist_path: &'a Path,
    pub helper_path: &'a Path,
    pub support_path: &'a Path,
    pub core_dir: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlistFacts {
    pub label: String,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub support_path: PathBuf,
    pub core_dir: PathBuf,
    pub singbox: Option<PathBuf>,
    pub confdir: Option<PathBuf>,
    pub upgrade_closed: bool,
    pub exit_timeout_seconds: Option<u32>,
    pub keep_alive: bool,
    pub run_at_load: bool,
    pub inferred_program: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedFacts {
    pub pid: u32,
    pub exit_timeout_seconds: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlist {
    #[serde(rename = "Label")]
    label: String,
    #[serde(rename = "Program")]
    program: Option<String>,
    #[serde(rename = "ProgramArguments")]
    arguments: Vec<String>,
    #[serde(rename = "KeepAlive")]
    keep_alive: bool,
    #[serde(rename = "RunAtLoad")]
    run_at_load: bool,
    #[serde(rename = "ExitTimeOut")]
    exit_timeout: Option<u32>,
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

fn label_valid(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 255
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn one_line(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && !value
            .chars()
            .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
}

fn absolute(value: &str) -> io::Result<PathBuf> {
    if !one_line(value)
        || value.len() > 4096
        || !value.starts_with('/')
        || value == "/"
        || value
            .split('/')
            .skip(1)
            .any(|c| matches!(c, "" | "." | ".."))
    {
        return Err(invalid("non-canonical absolute configuration path"));
    }
    Ok(PathBuf::from(value))
}

fn path_text(path: &Path) -> io::Result<&str> {
    let value = path
        .to_str()
        .ok_or_else(|| invalid("non-UTF8 configuration path"))?;
    absolute(value)?;
    Ok(value)
}

fn validate_expected(expected: &Expected<'_>) -> io::Result<()> {
    if !label_valid(expected.label) {
        return Err(invalid("invalid expected label"));
    }
    for path in [
        expected.plist_path,
        expected.helper_path,
        expected.support_path,
        expected.core_dir,
    ] {
        path_text(path)?;
    }
    Ok(())
}

/// Read the caller's already validated held object, without reopening its data
/// or changing the shared file offset. Recheck identity and coherent contents.
#[cfg(unix)]
pub fn read_protected_plist(
    held: &crate::protected_install_path::ProtectedPath,
    expected: &Expected<'_>,
) -> io::Result<PlistFacts> {
    use std::os::unix::fs::{FileExt, MetadataExt};
    if held.path != expected.plist_path {
        return Err(invalid("held plist is not the expected path"));
    }
    held.unchanged()?;
    let before = held.file.metadata()?;
    if !before.is_file() || before.len() == 0 || before.len() > MAX_PLIST as u64 {
        return Err(invalid("invalid held plist size or type"));
    }
    let mut bytes = vec![0; before.len() as usize];
    held.file.read_exact_at(&mut bytes, 0)?;
    let mut second = vec![0; bytes.len()];
    held.file.read_exact_at(&mut second, 0)?;
    let after = held.file.metadata()?;
    let stamp = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.uid(),
            m.gid(),
            m.mode(),
            m.nlink(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if stamp(&before) != stamp(&after) || bytes != second {
        return Err(invalid("held plist changed while reading"));
    }
    held.unchanged()?;
    parse_plist(&bytes, expected)
}

pub fn parse_plist(bytes: &[u8], expected: &Expected<'_>) -> io::Result<PlistFacts> {
    validate_expected(expected)?;
    if bytes.is_empty() || bytes.len() > MAX_PLIST {
        return Err(invalid("invalid plist size"));
    }
    let raw: RawPlist = plist::from_bytes(bytes)
        .map_err(|_| invalid("unsupported or malformed migration plist"))?;
    if raw.label != expected.label || raw.arguments.is_empty() || raw.arguments.len() > 10 {
        return Err(invalid("plist label or argument count mismatch"));
    }
    let program = absolute(&raw.arguments[0])?;
    if program != expected.helper_path
        || raw
            .program
            .as_deref()
            .map(absolute)
            .transpose()?
            .as_ref()
            .is_some_and(|p| p != expected.helper_path)
    {
        return Err(invalid("plist executable mismatch"));
    }
    let mut flags = BTreeMap::new();
    let mut upgrade_closed = false;
    let mut index = 1;
    while index < raw.arguments.len() {
        let flag = raw.arguments[index].as_str();
        if flag == "--upgrade-closed" {
            if upgrade_closed {
                return Err(invalid("duplicate daemon flag"));
            }
            upgrade_closed = true;
            index += 1;
            continue;
        }
        if !matches!(flag, "--support" | "--coredir" | "--singbox" | "--confdir") {
            return Err(invalid("unsupported daemon flag"));
        }
        let value = raw
            .arguments
            .get(index + 1)
            .ok_or_else(|| invalid("missing daemon flag value"))?;
        if flags.insert(flag, absolute(value)?).is_some() {
            return Err(invalid("duplicate daemon flag"));
        }
        index += 2;
    }
    if flags.get("--support").map(PathBuf::as_path) != Some(expected.support_path)
        || flags.get("--coredir").map(PathBuf::as_path) != Some(expected.core_dir)
    {
        return Err(invalid("support or core directory mismatch"));
    }
    let singbox = flags.remove("--singbox");
    if let Some(path) = &singbox {
        if path != &expected.core_dir.join("sing-box") {
            let relative = path
                .strip_prefix(expected.core_dir.join("versions"))
                .map_err(|_| invalid("singbox outside managed executable layout"))?;
            let parts: Vec<_> = relative.iter().collect();
            if parts.len() != 2
                || parts[1] != "sing-box"
                || parts[0]
                    .to_str()
                    .is_none_or(|id| CoreId::parse(id).is_err())
            {
                return Err(invalid("invalid managed content executable path"));
            }
        }
    }
    let confdir = flags.remove("--confdir");
    drop(flags);
    Ok(PlistFacts {
        label: raw.label,
        program,
        support_path: expected.support_path.to_owned(),
        core_dir: expected.core_dir.to_owned(),
        singbox,
        confdir,
        upgrade_closed,
        exit_timeout_seconds: raw.exit_timeout,
        keep_alive: raw.keep_alive,
        run_at_load: raw.run_at_load,
        inferred_program: raw.program.is_none(),
        arguments: raw.arguments,
    })
}

fn checked_lines(text: &str) -> io::Result<Vec<&str>> {
    if text.is_empty()
        || text.len() > MAX_PRINT
        || !text.ends_with('\n')
        || text.chars().any(|c| {
            (c.is_control() && c != '\n' && c != '\t') || c == '\u{2028}' || c == '\u{2029}'
        })
    {
        return Err(invalid("incomplete or invalid launchctl print"));
    }
    let lines: Vec<_> = text.lines().collect();
    if lines.len() > 4096 || lines.iter().any(|line| line.len() > 8192) {
        return Err(invalid("launchctl print limits exceeded"));
    }
    Ok(lines)
}

fn pair(line: &str) -> io::Result<(&str, &str)> {
    let (key, value) = line
        .split_once(" = ")
        .ok_or_else(|| invalid("unsupported launchctl field shape"))?;
    if !one_line(key) || !one_line(value) {
        return Err(invalid("ambiguous launchctl field"));
    }
    Ok((key, value))
}

fn numeric(value: &str) -> io::Result<u32> {
    if value.is_empty()
        || !value.bytes().all(|b| b.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(invalid("invalid launchctl numeric value"));
    }
    value
        .parse()
        .map_err(|_| invalid("launchctl numeric value out of range"))
}

fn display_key(key: &str) -> bool {
    matches!(
        key,
        "active count"
            | "path"
            | "type"
            | "state"
            | "program"
            | "domain"
            | "minimum runtime"
            | "exit timeout"
            | "runs"
            | "pid"
            | "immediate reason"
            | "forks"
            | "execs"
            | "initialized"
            | "trampolined"
            | "started suspended"
            | "proxy started suspended"
            | "checked allocations"
            | "checked allocations reason"
            | "checked allocations flags"
            | "last exit code"
            | "spawn type"
            | "jetsam priority"
            | "jetsam memory limit (active)"
            | "jetsam memory limit (inactive)"
            | "jetsamproperties category"
            | "jetsam thread limit"
            | "cpumon"
            | "job state"
            | "sanitizer flags"
            | "properties"
    )
}

/// Validate one complete running system job. Nested coalition `pid`, `state`,
/// etc. never satisfy top-level fields. Only the observed harmless environment
/// keys PATH, OSLogRateLimit and XPC_SERVICE_NAME are allowed in their respective
/// blocks; their values are not executable identity evidence. Any other loaded
/// environment (including DYLD/proxy/debug settings) rejects the job.
pub fn validate_loaded_print(
    text: &str,
    plist: &PlistFacts,
    expected: &Expected<'_>,
) -> io::Result<LoadedFacts> {
    validate_expected(expected)?;
    let lines = checked_lines(text)?;
    if lines.first().copied() != Some(format!("system/{} = {{", expected.label).as_str())
        || lines.last().copied() != Some("}")
        || plist.label != expected.label
        || plist.program != expected.helper_path
    {
        return Err(invalid("loaded job root mismatch"));
    }
    let mut fields = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut arguments = Vec::new();
    let mut block: Option<&str> = None;
    let mut nested_seen = BTreeSet::new();
    for line in &lines[1..lines.len() - 1] {
        if line.is_empty() {
            if block.is_some() {
                return Err(invalid("empty nested launchctl line"));
            }
            continue;
        }
        if let Some(name) = block {
            if *line == "\t}" {
                block = None;
                continue;
            }
            let value = line
                .strip_prefix("\t\t")
                .filter(|value| one_line(value))
                .ok_or_else(|| invalid("invalid nested launchctl indentation"))?;
            if name == "arguments" {
                arguments.push(value.to_owned());
            } else if matches!(name, "environment" | "default environment") {
                let (key, value) = value
                    .split_once(" => ")
                    .ok_or_else(|| invalid("invalid loaded environment"))?;
                let supported = if name == "default environment" {
                    key == "PATH"
                } else {
                    matches!(key, "OSLogRateLimit" | "XPC_SERVICE_NAME")
                };
                if !supported || !one_line(value) || !nested_seen.insert(key) {
                    return Err(invalid("unsupported or duplicate loaded environment"));
                }
            } else {
                let (key, value) = pair(value)?;
                if !matches!(key, "ID" | "type" | "state" | "active count" | "name")
                    || value == "{"
                    || !nested_seen.insert(key)
                {
                    return Err(invalid("unsupported or duplicate coalition field"));
                }
            }
            continue;
        }
        let value = line
            .strip_prefix('\t')
            .ok_or_else(|| invalid("invalid top-level launchctl indentation"))?;
        let (key, value) = pair(value)?;
        if !seen.insert(key) {
            return Err(invalid("duplicate top-level launchctl field"));
        }
        if value == "{" {
            if !matches!(
                key,
                "arguments"
                    | "environment"
                    | "default environment"
                    | "resource coalition"
                    | "jetsam coalition"
            ) {
                return Err(invalid("unsupported launchctl block"));
            }
            block = Some(key);
            nested_seen.clear();
        } else {
            if !display_key(key) {
                return Err(invalid("unsupported launchctl field"));
            }
            fields.insert(key, value);
        }
    }
    if block.is_some() || arguments != plist.arguments {
        return Err(invalid("incomplete or mismatched loaded arguments"));
    }
    let required = |key| {
        fields
            .get(key)
            .copied()
            .ok_or_else(|| invalid("missing authoritative launchctl field"))
    };
    for (key, wanted) in [
        ("path", path_text(expected.plist_path)?),
        ("program", path_text(expected.helper_path)?),
        ("type", "LaunchDaemon"),
        ("state", "running"),
        ("domain", "system"),
    ] {
        if required(key)? != wanted {
            return Err(invalid("loaded job does not match protected plist"));
        }
    }
    for (key, wanted) in [
        ("started suspended", "0"),
        ("proxy started suspended", "0"),
        ("sanitizer flags", "0x0"),
        ("spawn type", "daemon (3)"),
        ("job state", "running"),
    ] {
        if required(key)? != wanted {
            return Err(invalid("unsupported loaded process configuration"));
        }
    }
    let mut properties = BTreeSet::new();
    for value in required("properties")?.split(" | ") {
        if !properties.insert(value) {
            return Err(invalid("duplicate loaded property"));
        }
    }
    let expected_properties: BTreeSet<_> = [
        (plist.keep_alive, "keepalive"),
        (plist.run_at_load, "runatload"),
        (plist.inferred_program, "inferred program"),
    ]
    .into_iter()
    .filter_map(|(present, value)| present.then_some(value))
    .collect();
    if properties != expected_properties {
        return Err(invalid("unsupported or mismatched loaded properties"));
    }
    let pid = numeric(required("pid")?)?;
    let exit_timeout_seconds = numeric(required("exit timeout")?)?;
    if pid <= 1
        || pid > i32::MAX as u32
        || plist
            .exit_timeout_seconds
            .is_some_and(|value| value != exit_timeout_seconds)
    {
        return Err(invalid("invalid loaded PID or mismatched exit timeout"));
    }
    Ok(LoadedFacts {
        pid,
        exit_timeout_seconds,
    })
}

/// A missing target is an error, not evidence that launchd disabled the job.
/// Only the exact quoted top-level target's explicit boolean is accepted.
pub fn parse_disabled_print(text: &str, label: &str) -> io::Result<bool> {
    parse_disabled_state(text, label)?
        .ok_or_else(|| invalid("target missing from disabled-services print"))
}

/// `None` only reports omission from a complete successful disabled-services
/// table. It is never proof that a job is disabled. The guard alone interprets
/// the initial enabled default together with its closed, validated plist schema.
pub fn parse_disabled_state(text: &str, label: &str) -> io::Result<Option<bool>> {
    if !label_valid(label) {
        return Err(invalid("invalid disabled target label"));
    }
    let lines = checked_lines(text)?;
    if lines.first().copied() != Some("disabled services = {") || lines.last().copied() != Some("}")
    {
        return Err(invalid("invalid disabled-services root"));
    }
    let mut seen = BTreeSet::new();
    let mut disabled = None;
    for line in &lines[1..lines.len() - 1] {
        let (key, value) = line
            .strip_prefix('\t')
            .and_then(|line| line.split_once(" => "))
            .ok_or_else(|| invalid("invalid disabled-services entry"))?;
        let name = key
            .strip_prefix('"')
            .and_then(|key| key.strip_suffix('"'))
            .filter(|name| label_valid(name))
            .ok_or_else(|| invalid("invalid disabled-services label"))?;
        let flag = match value {
            "true" => true,
            "false" => false,
            _ => return Err(invalid("invalid disabled-services boolean")),
        };
        if !seen.insert(name) {
            return Err(invalid("duplicate disabled-services entry"));
        }
        if name == label {
            disabled = Some(flag);
        }
    }
    Ok(disabled)
}

/// Positive domain-table absence, independent of any failed `print system/job`.
/// The caller must pass the original complete successful `print system` stdout,
/// never a filtered table or a document with synthesized closing delimiters.
/// Unrelated domain sections are structurally checked, not interpreted as job
/// configuration. Only a unique, direct `services` table is authoritative.
pub fn parse_system_services_absent(text: &str, label: &str) -> io::Result<bool> {
    if !label_valid(label) {
        return Err(invalid("invalid services target label"));
    }
    let lines = checked_lines(text)?;
    if lines.first().copied() != Some("system = {") || lines.last().copied() != Some("}") {
        return Err(invalid("invalid system domain root"));
    }
    let mut depth = 1usize;
    let mut services = false;
    let mut saw_services = false;
    let mut seen = BTreeSet::new();
    for line in &lines[1..lines.len() - 1] {
        if line.is_empty() {
            continue;
        }
        let tabs = line.bytes().take_while(|b| *b == b'\t').count();
        let value = &line[tabs..];
        if value == "}" {
            if depth <= 1 || tabs + 1 != depth {
                return Err(invalid("unbalanced system domain block"));
            }
            depth -= 1;
            if services && depth == 1 {
                services = false;
            }
            continue;
        }
        if tabs != depth || depth > 16 {
            return Err(invalid("invalid system domain indentation"));
        }
        if services {
            if depth != 2 {
                return Err(invalid("nested services table"));
            }
            let fields: Vec<_> = value.split_whitespace().collect();
            if fields.len() != 3 || numeric(fields[0])? > i32::MAX as u32 || !label_valid(fields[2])
            {
                return Err(invalid("unrecognized services row"));
            }
            if !matches!(fields[1], "-" | "(pe)") {
                let number = fields[1]
                    .parse::<i32>()
                    .map_err(|_| invalid("invalid services exit status"))?;
                if number.to_string() != fields[1] {
                    return Err(invalid("non-canonical services exit status"));
                }
            }
            if !seen.insert(fields[2]) {
                return Err(invalid("duplicate services label"));
            }
            continue;
        }
        if value.ends_with(" = {") {
            let name = value.strip_suffix(" = {").unwrap();
            if !one_line(name) {
                return Err(invalid("invalid system domain block name"));
            }
            if depth == 1 && name == "services" {
                if saw_services {
                    return Err(invalid("duplicate top-level services table"));
                }
                services = true;
                saw_services = true;
            }
            depth += 1;
        } else if !one_line(value) || value.contains(['{', '}']) {
            return Err(invalid("unsupported system domain line"));
        }
    }
    if depth != 1 || services || !saw_services {
        return Err(invalid("missing or incomplete system services table"));
    }
    Ok(!seen.contains(label))
}

#[cfg(test)]
mod tests;
