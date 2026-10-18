// The formal App builder verifies the exact component slice/source/BuildID
// before Cargo. Check that same static input against the existing wire table;
// iOS must never fall back to a desktop executable during release compilation.
#[allow(dead_code)]
mod apple_wire_check {
    use super::proto_wire_check::{WireVerdict, verdict_for_core_bytes};
    use std::path::Path;

    pub fn check_bytes(bytes: &[u8]) -> Result<(), String> {
        if !bytes.starts_with(b"!<arch>\n") || bytes.len() > 1024 * 1024 * 1024 {
            return Err("actual bounded Apple static archive required".into());
        }
        match verdict_for_core_bytes(bytes) {
            WireVerdict::Match => Ok(()),
            WireVerdict::Mismatch(report) => Err(format!("Apple static wire mismatch: {report}")),
            WireVerdict::Unobservable(report) => {
                Err(format!("Apple static descriptor unavailable: {report}"))
            }
        }
    }

    fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, String> {
        let raw = bytes
            .get(offset..offset + 4)
            .ok_or("truncated Apple object")?;
        Ok(u32::from_le_bytes(
            raw.try_into().map_err(|_| "invalid Apple word")?,
        ))
    }

    fn decimal(raw: &[u8]) -> Result<usize, String> {
        let text = std::str::from_utf8(raw)
            .map_err(|_| "non-ASCII ar field")?
            .trim();
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("invalid ar decimal field".into());
        }
        text.parse().map_err(|_| "ar field overflow".into())
    }

    pub fn check_bytes_for_target(bytes: &[u8], target: &str) -> Result<(), String> {
        let (cpu, subtype, platform) = match target {
            "aarch64-apple-ios" => (0x0100_000c, 0, 2),
            "aarch64-apple-ios-sim" => (0x0100_000c, 0, 7),
            _ => return Err("unsupported actual Cargo Apple target".into()),
        };
        if !bytes.starts_with(b"!<arch>\n") || bytes.len() > 1024 * 1024 * 1024 {
            // The builder extracts a fully inspected, receipt-bound target slice.
            // Never scan the whole fat binary and accidentally accept another CPU's descriptor.
            return Err("actual bounded selected thin Apple static archive required".into());
        }
        let mut position = 8_usize;
        let mut code_count = 0_usize;
        let mut platform_count = 0_usize;
        while position < bytes.len() {
            let header = bytes
                .get(position..position + 60)
                .ok_or("truncated ar header")?;
            if &header[58..] != b"`\n" || !header[..58].iter().all(|byte| (32..127).contains(byte))
            {
                return Err("invalid BSD ar header".into());
            }
            let name_field = std::str::from_utf8(&header[..16])
                .map_err(|_| "ar name ASCII")?
                .trim();
            let name_length = decimal(
                name_field
                    .strip_prefix("#1/")
                    .ok_or("BSD ar name required")?
                    .as_bytes(),
            )?;
            let length = decimal(&header[48..58])?;
            let start = position.checked_add(60).ok_or("ar offset overflow")?;
            let end = start.checked_add(length).ok_or("ar length overflow")?;
            let body = bytes.get(start..end).ok_or("truncated ar member")?;
            let name = body.get(..name_length).ok_or("ar name bounds")?;
            let name = name
                .split(|byte| *byte == 0)
                .next()
                .ok_or("ar name missing")?;
            let object = body.get(name_length..).ok_or("ar payload bounds")?;
            // Xcode's actual thin archives use the unsorted BSD spelling too.
            // The App builder already validates the receipt-bound table; only
            // these two exact metadata names bypass the Mach-O object check.
            if name != b"__.SYMDEF SORTED" && name != b"__.SYMDEF" {
                if object.get(..4) != Some(b"\xcf\xfa\xed\xfe")
                    || u32_at(object, 4)? != cpu
                    || u32_at(object, 8)? != subtype
                    || u32_at(object, 12)? != 1
                {
                    return Err(
                        "selected Apple object CPU/subtype/type differs from Cargo target".into(),
                    );
                }
                code_count += 1;
                let count = u32_at(object, 16)? as usize;
                let command_end = 32_usize
                    .checked_add(u32_at(object, 20)? as usize)
                    .ok_or("command overflow")?;
                if command_end > object.len() || count > (command_end - 32) / 8 {
                    return Err("Apple load-command bounds".into());
                }
                let mut cursor = 32_usize;
                for _ in 0..count {
                    let command = u32_at(object, cursor)?;
                    let size = u32_at(object, cursor + 4)? as usize;
                    let next = cursor.checked_add(size).ok_or("command size overflow")?;
                    if size < 8 || next > command_end {
                        return Err("Apple load-command layout".into());
                    }
                    if command == 0x32 {
                        if size < 24 || u32_at(object, cursor + 8)? != platform {
                            return Err(
                                "selected Apple object platform differs from Cargo target".into()
                            );
                        }
                        platform_count += 1;
                    }
                    cursor = next;
                }
                if cursor != command_end {
                    return Err("Apple load-command count differs".into());
                }
            } else if code_count != 0 {
                return Err("BSD ar symbol table position differs".into());
            }
            position = end.checked_add(length % 2).ok_or("ar padding overflow")?;
            if length % 2 == 1 && bytes.get(end) != Some(&b'\n') {
                return Err("ar padding differs".into());
            }
        }
        if position != bytes.len() || code_count == 0 || platform_count == 0 {
            return Err("selected Apple code/platform inventory missing".into());
        }
        check_bytes(bytes)
    }

    pub fn check_apple_archive(path: &Path) -> Result<(), String> {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 * 1024 {
            return Err("regular bounded selected Apple archive required".into());
        }
        let target = std::env::var("TARGET").map_err(|_| "actual Cargo target is required")?;
        check_bytes_for_target(
            &std::fs::read(path).map_err(|error| error.to_string())?,
            &target,
        )
    }
}
