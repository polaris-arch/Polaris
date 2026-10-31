//! Bounded static PE import subset. Resources (including activation manifests)
//! and delay imports are deliberately unsupported until separately audited.
use super::*;

fn bad() -> AdmissionError {
    AdmissionError::new(
        AdmissionErrorKind::LoaderPolicy,
        "unsupported Windows PE loader closure",
    )
}
fn u16_at(b: &[u8], p: usize) -> Result<u16, AdmissionError> {
    Ok(u16::from_le_bytes(
        b.get(p..p.checked_add(2).ok_or_else(bad)?)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?,
    ))
}
fn u32_at(b: &[u8], p: usize) -> Result<u32, AdmissionError> {
    Ok(u32::from_le_bytes(
        b.get(p..p.checked_add(4).ok_or_else(bad)?)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?,
    ))
}
pub(super) fn imports(b: &[u8]) -> Result<Vec<String>, AdmissionError> {
    if b.get(..2) != Some(b"MZ") {
        return Err(bad());
    }
    let p = u32_at(b, 60)? as usize;
    if b.get(p..p.checked_add(4).ok_or_else(bad)?) != Some(b"PE\0\0") || u16_at(b, p + 4)? != 0x8664
    {
        return Err(bad());
    }
    let count = u16_at(b, p + 6)? as usize;
    let opt_size = u16_at(b, p + 20)? as usize;
    let opt = p.checked_add(24).ok_or_else(bad)?;
    if count == 0
        || count > 96
        || opt_size < 240
        || u16_at(b, opt)? != 0x20b
        || u32_at(b, opt + 108)? < 14
    {
        return Err(bad());
    }
    // Do not imply that PreferSystem32 alone defeats activation contexts.
    for entry in [2, 13] {
        if u32_at(b, opt + 112 + entry * 8)? != 0 || u32_at(b, opt + 116 + entry * 8)? != 0 {
            return Err(bad());
        }
    }
    let section = opt.checked_add(opt_size).ok_or_else(bad)?;
    let offset = |rva: u32| -> Result<usize, AdmissionError> {
        let mut found = None;
        for i in 0..count {
            let s = section.checked_add(i * 40).ok_or_else(bad)?;
            let start = u32_at(b, s + 12)?;
            let size = u32_at(b, s + 16)?;
            let raw = u32_at(b, s + 20)?;
            if rva >= start && rva - start < size {
                let o = raw.checked_add(rva - start).ok_or_else(bad)? as usize;
                if o >= b.len() || found.replace(o).is_some() {
                    return Err(bad());
                }
            }
        }
        found.ok_or_else(bad)
    };
    let start = u32_at(b, opt + 120)?;
    let size = u32_at(b, opt + 124)?;
    if start == 0 && size == 0 {
        return Ok(Vec::new());
    }
    if size < 20 || size > 64 * 1024 {
        return Err(bad());
    }
    let mut result = Vec::new();
    for i in 0..(size / 20).min(256) {
        let rva = start.checked_add(i * 20).ok_or_else(bad)?;
        let o = offset(rva)?;
        let row = b
            .get(o..o.checked_add(20).ok_or_else(bad)?)
            .ok_or_else(bad)?;
        if row.iter().all(|x| *x == 0) {
            return Ok(result);
        }
        let n = offset(u32_at(b, o + 12)?)?;
        let tail = b
            .get(n..n.saturating_add(260).min(b.len()))
            .ok_or_else(bad)?;
        let end = tail.iter().position(|x| *x == 0).ok_or_else(bad)?;
        let text = std::str::from_utf8(&tail[..end])
            .map_err(|_| bad())?
            .to_ascii_lowercase();
        if text.len() < 5
            || !text.ends_with(".dll")
            || text
                .bytes()
                .any(|c| !c.is_ascii_alphanumeric() && c != b'.' && c != b'-' && c != b'_')
            || text.contains("..")
        {
            return Err(bad());
        }
        if !result.contains(&text) {
            result.push(text);
        }
    }
    Err(bad())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_and_non_pe_inputs_never_prove_a_loader() {
        for b in [
            vec![],
            b"ordinary data".to_vec(),
            b"MZ".to_vec(),
            vec![0; 4096],
        ] {
            assert!(imports(&b).is_err());
        }
    }
}
