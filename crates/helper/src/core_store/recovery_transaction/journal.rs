//! A bounded immutable plan. No record can supply an arbitrary filesystem path.

use super::super::invalid;
use super::objects::{leaf, Directory, Identity, Regular, MAX_BYTES, MAX_ENTRIES};
use polaris_helper_proto::core_manifest::CoreId;
use std::collections::BTreeMap;
use std::io;

pub(super) const MAX_PLAN: usize = 32 * 1024;
const HEADER: &str = "polaris-core-repair-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Plan {
    pub selected: CoreId,
    pub root: Identity,
    pub transaction: Identity,
    pub lease: Identity,
    pub stage: String,
    pub candidate: Directory,
    pub candidate_manifest: Regular,
    pub original: Option<Directory>,
    pub original_manifest: Option<Regular>,
}

impl Plan {
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        leaf(&self.stage)?;
        let mut value = format!(
            "{HEADER}\n{}\n{}\n{}\n{}\n{}\n",
            self.selected.as_str(),
            self.root.0,
            self.transaction.0,
            self.lease.0,
            hex::encode(&self.stage)
        );
        write_directory(&mut value, Some(&self.candidate));
        write_regular(&mut value, Some(&self.candidate_manifest));
        write_directory(&mut value, self.original.as_ref());
        write_regular(&mut value, self.original_manifest.as_ref());
        let digest = crate::core_hash::sha256_hex(value.as_bytes());
        value.push_str(&digest);
        value.push('\n');
        if value.len() > MAX_PLAN {
            return Err(invalid("repair plan exceeds bound"));
        }
        // One strict parser for new and resumed records.
        if Self::parse(value.as_bytes())? != *self {
            return Err(invalid("repair plan round trip differs"));
        }
        Ok(value.into_bytes())
    }

    pub fn parse(raw: &[u8]) -> io::Result<Self> {
        if raw.len() > MAX_PLAN || !raw.ends_with(b"\n") {
            return Err(invalid("invalid repair plan length"));
        }
        let text = std::str::from_utf8(raw).map_err(|_| invalid("invalid repair plan encoding"))?;
        let (body, digest) = text[..text.len() - 1]
            .rsplit_once('\n')
            .ok_or_else(|| invalid("missing repair checksum"))?;
        let body = format!("{body}\n");
        if crate::core_hash::sha256_hex(body.as_bytes()) != digest {
            return Err(invalid("repair checksum differs"));
        }
        let mut input = body.lines();
        if next(&mut input)? != HEADER {
            return Err(invalid("unknown repair schema"));
        }
        let selected = CoreId::parse(next(&mut input)?).map_err(invalid)?;
        let root = parse_identity(next(&mut input)?)?;
        let transaction = parse_identity(next(&mut input)?)?;
        let lease = parse_identity(next(&mut input)?)?;
        let stage = decode_name(next(&mut input)?)?;
        let candidate =
            read_directory(&mut input)?.ok_or_else(|| invalid("missing repair candidate"))?;
        let candidate_manifest =
            read_regular(&mut input)?.ok_or_else(|| invalid("missing candidate manifest"))?;
        let original = read_directory(&mut input)?;
        let original_manifest = read_regular(&mut input)?;
        if input.next().is_some() {
            return Err(invalid("trailing repair fields"));
        }
        let value = Self {
            selected,
            root,
            transaction,
            lease,
            stage,
            candidate,
            candidate_manifest,
            original,
            original_manifest,
        };
        if value.candidate_manifest.bytes > 64 * 1024
            || value
                .original_manifest
                .as_ref()
                .is_some_and(|file| file.bytes > 64 * 1024)
            || value.stage.starts_with("repair-")
            || value.original.as_ref().is_some_and(|old| {
                old.files
                    .keys()
                    .any(|name| !value.candidate.files.contains_key(name))
            })
        {
            return Err(invalid("invalid repair plan bounds"));
        }
        Ok(value)
    }

    pub fn commit_bytes(raw: &[u8]) -> Vec<u8> {
        format!(
            "polaris-core-repair-commit-v1\n{}\n",
            crate::core_hash::sha256_hex(raw)
        )
        .into_bytes()
    }
}

fn write_regular(output: &mut String, file: Option<&Regular>) {
    match file {
        None => output.push_str("-\n"),
        Some(file) => output.push_str(&format!(
            "{} {} {}\n",
            file.identity.0, file.bytes, file.sha256
        )),
    }
}

fn write_directory(output: &mut String, directory: Option<&Directory>) {
    match directory {
        None => output.push_str("-\n"),
        Some(directory) => {
            output.push_str(&format!(
                "{} {}\n",
                directory.identity.0,
                directory.files.len()
            ));
            for (name, file) in &directory.files {
                output.push_str(&hex::encode(name));
                output.push('\n');
                write_regular(output, Some(file));
            }
        }
    }
}

fn next<'a>(input: &mut impl Iterator<Item = &'a str>) -> io::Result<&'a str> {
    input.next().ok_or_else(|| invalid("truncated repair plan"))
}

fn parse_identity(raw: &str) -> io::Result<Identity> {
    let (prefix, rest) = raw
        .split_at_checked(1)
        .ok_or_else(|| invalid("empty object identity"))?;
    let parts: Vec<_> = rest.split('-').collect();
    if !((prefix == "u" && parts.len() == 2) || (prefix == "w" && parts.len() == 3))
        || parts.iter().any(|part| {
            part.is_empty()
                || part.len() > 16
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
    {
        return Err(invalid("invalid repair object identity"));
    }
    Ok(Identity(raw.into()))
}

fn decode_name(raw: &str) -> io::Result<String> {
    if raw.len() > 510 {
        return Err(invalid("repair name exceeds bound"));
    }
    let value =
        String::from_utf8(hex::decode(raw).map_err(|_| invalid("invalid repair name encoding"))?)
            .map_err(|_| invalid("invalid repair name text"))?;
    leaf(&value)?;
    Ok(value)
}

fn read_regular<'a>(input: &mut impl Iterator<Item = &'a str>) -> io::Result<Option<Regular>> {
    let raw = next(input)?;
    if raw == "-" {
        return Ok(None);
    }
    let parts: Vec<_> = raw.split(' ').collect();
    if parts.len() != 3 {
        return Err(invalid("invalid repair file record"));
    }
    let identity = parse_identity(parts[0])?;
    let bytes: u64 = parts[1]
        .parse()
        .map_err(|_| invalid("invalid repair size"))?;
    if bytes > MAX_BYTES || parts[1] != bytes.to_string() {
        return Err(invalid("repair size exceeds bound"));
    }
    if parts[2].len() != 64 || CoreId::parse(parts[2]).is_err() {
        return Err(invalid("invalid repair digest"));
    }
    Ok(Some(Regular {
        identity,
        bytes,
        sha256: parts[2].into(),
    }))
}

fn read_directory<'a>(input: &mut impl Iterator<Item = &'a str>) -> io::Result<Option<Directory>> {
    let raw = next(input)?;
    if raw == "-" {
        return Ok(None);
    }
    let parts: Vec<_> = raw.split(' ').collect();
    if parts.len() != 2 {
        return Err(invalid("invalid repair directory record"));
    }
    let identity = parse_identity(parts[0])?;
    let count: usize = parts[1]
        .parse()
        .map_err(|_| invalid("invalid repair member count"))?;
    if count > MAX_ENTRIES || parts[1] != count.to_string() {
        return Err(invalid("repair member count exceeds bound"));
    }
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for _ in 0..count {
        let name = decode_name(next(input)?)?;
        let file = read_regular(input)?.ok_or_else(|| invalid("missing repair member"))?;
        total = total
            .checked_add(file.bytes)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(|| invalid("repair members exceed size bound"))?;
        if files.insert(name, file).is_some() {
            return Err(invalid("duplicate repair member"));
        }
    }
    Ok(Some(Directory { identity, files }))
}
