//! Keep SwiftPM's resolver and generated Tauri package inside Cargo's OUT_DIR.
//! Package.swift/Package.resolved/Sources remain exact inputs; resolver output
//! never becomes an untracked file in the frozen checkout.
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_ENTRIES: usize = 256;
const MAX_BYTES: u64 = 4 * 1024 * 1024;

fn reject() -> io::Error {
    io::Error::other("Swift package staging requires bounded ordinary files/directories")
}

fn directory(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(reject());
    }
    Ok(())
}

fn files(path: &Path, entries: &mut usize, bytes: &mut u64) -> io::Result<Vec<(PathBuf, Vec<u8>)>> {
    *entries += 1;
    if *entries > MAX_ENTRIES {
        return Err(reject());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        let mut result = Vec::new();
        for entry in fs::read_dir(path)? {
            result.extend(files(&entry?.path(), entries, bytes)?);
        }
        Ok(result)
    } else if metadata.is_file() {
        let remaining = MAX_BYTES.checked_sub(*bytes).ok_or_else(reject)?;
        if metadata.len() > remaining {
            return Err(reject());
        }
        let mut content = Vec::new();
        fs::File::open(path)?
            .take(remaining + 1)
            .read_to_end(&mut content)?;
        *bytes += content.len() as u64;
        if *bytes > MAX_BYTES || content.len() as u64 != metadata.len() {
            return Err(reject());
        }
        Ok(vec![(path.to_owned(), content)])
    } else {
        Err(reject())
    }
}

fn owned_directory(path: &Path) -> io::Result<()> {
    if !path.try_exists()? {
        fs::create_dir(path)?;
    }
    directory(path)
}

pub(crate) fn stage(source: &Path, out: &Path) -> io::Result<PathBuf> {
    directory(source)?;
    let source = source.canonicalize()?;
    let out = out.canonicalize()?;
    if out.starts_with(&source) || source.starts_with(&out) {
        return Err(reject());
    }
    directory(&source.join("Sources"))?;
    for name in ["Package.swift", "Package.resolved"] {
        if !fs::symlink_metadata(source.join(name))?.is_file() {
            return Err(reject());
        }
    }
    let (mut entries, mut bytes) = (0, 0);
    let mut inputs = files(&source.join("Package.swift"), &mut entries, &mut bytes)?;
    inputs.extend(files(
        &source.join("Package.resolved"),
        &mut entries,
        &mut bytes,
    )?);
    inputs.extend(files(&source.join("Sources"), &mut entries, &mut bytes)?);
    let root = out.join("polaris-ios-package");
    owned_directory(&root)?;
    let package = root.join("ios");
    owned_directory(&package)?;
    // Refuse redirected retained output before restoring the source-owned lock.
    let resolved = package.join("Package.resolved");
    if fs::symlink_metadata(&resolved).is_ok() {
        if !fs::symlink_metadata(&resolved)?.is_file() {
            return Err(reject());
        }
        files(&resolved, &mut 0, &mut 0)?;
    }
    let sources = package.join("Sources");
    if fs::symlink_metadata(&sources).is_ok() {
        directory(&sources)?;
        files(&sources, &mut 0, &mut 0)?;
        fs::remove_dir_all(&sources)?;
    }
    fs::create_dir(&sources)?;
    for (path, content) in inputs {
        let destination = package.join(path.strip_prefix(&source).map_err(|_| reject())?);
        if fs::symlink_metadata(&destination).is_ok()
            && !fs::symlink_metadata(&destination)?.is_file()
        {
            return Err(reject());
        }
        fs::create_dir_all(destination.parent().ok_or_else(reject)?)?;
        fs::write(destination, content)?;
    }
    Ok(package)
}

pub(crate) fn verify_resolution(source: &Path, package: &Path) -> io::Result<()> {
    fn lock(root: &Path) -> io::Result<serde_json::Value> {
        let path = root.join("Package.resolved");
        if !fs::symlink_metadata(&path)?.is_file() {
            return Err(reject());
        }
        let rows = files(&path, &mut 0, &mut 0)?;
        serde_json::from_slice(&rows[0].1).map_err(|_| reject())
    }
    // SwiftPM serializes v1 for tools 5.3, v2 for 5.6, and v3 after 5.9.
    // Check each known envelope and the same complete pin, not byte formatting.
    fn pins(value: serde_json::Value) -> io::Result<serde_json::Value> {
        fn keys(value: &serde_json::Value, names: &[&str]) -> io::Result<()> {
            let object = value.as_object().ok_or_else(reject)?;
            if object.len() != names.len() || names.iter().any(|name| !object.contains_key(*name)) {
                return Err(reject());
            }
            Ok(())
        }
        let version = value["version"].as_u64().ok_or_else(reject)?;
        let rows = match version {
            1 => {
                keys(&value, &["version", "object"])?;
                keys(&value["object"], &["pins"])?;
                &value["object"]["pins"]
            }
            2 => {
                keys(&value, &["version", "pins"])?;
                &value["pins"]
            }
            3 => {
                keys(&value, &["version", "pins", "originHash"])?;
                let hash = value["originHash"].as_str().ok_or_else(reject)?;
                if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(reject());
                }
                &value["pins"]
            }
            _ => return Err(reject()),
        };
        let rows = rows.as_array().ok_or_else(reject)?;
        if rows.len() != 1 {
            return Err(reject());
        }
        let mut pin = rows[0].clone();
        if version == 1 {
            keys(&pin, &["package", "repositoryURL", "state"])?;
            if !matches!(pin["package"].as_str(), Some("SwiftRs" | "swift-rs")) {
                return Err(reject());
            }
            keys(&pin["state"], &["branch", "revision", "version"])?;
            if !pin["state"]["branch"].is_null() {
                return Err(reject());
            }
            pin["state"]
                .as_object_mut()
                .ok_or_else(reject)?
                .remove("branch");
            pin = serde_json::json!({"identity":"swift-rs", "kind":"remoteSourceControl",
                "location":pin["repositoryURL"], "state":pin["state"]});
        }
        keys(&pin, &["identity", "kind", "location", "state"])?;
        keys(&pin["state"], &["revision", "version"])?;
        let revision = pin["state"]["revision"].as_str().ok_or_else(reject)?;
        let version = pin["state"]["version"].as_str().ok_or_else(reject)?;
        if pin["identity"] != "swift-rs"
            || pin["kind"] != "remoteSourceControl"
            || pin["location"] != "https://github.com/Brendonovich/swift-rs"
            || revision.len() != 40
            || !revision.bytes().all(|b| b.is_ascii_hexdigit())
            || version.is_empty()
        {
            return Err(reject());
        }
        Ok(pin)
    }
    // No resolver body, URL or credential is printed on failure.
    if pins(lock(source)?)? != pins(lock(package)?)? {
        return Err(reject());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "polaris-swift-stage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("out")).unwrap();
            fs::create_dir(root.join("ios")).unwrap();
            fs::create_dir(root.join("ios/Sources")).unwrap();
            fs::write(root.join("ios/Package.swift"), b"unchanged manifest").unwrap();
            fs::write(
                root.join("ios/Package.resolved"),
                include_bytes!("ios/Package.resolved"),
            )
            .unwrap();
            fs::write(root.join("ios/Sources/Plugin.swift"), b"unchanged Swift").unwrap();
            Self(root)
        }
        fn stage(&self) -> io::Result<PathBuf> {
            stage(&self.0.join("ios"), &self.0.join("out"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn exact_inputs_and_resolver_cache_stay_outside_checkout() {
        let fixture = Fixture::new();
        let package = fixture.stage().unwrap();
        assert!(package.starts_with(fixture.0.join("out").canonicalize().unwrap()));
        for name in ["Package.swift", "Package.resolved", "Sources/Plugin.swift"] {
            assert_eq!(
                fs::read(package.join(name)).unwrap(),
                fs::read(fixture.0.join("ios").join(name)).unwrap()
            );
        }
        // A resolver write belongs to the build slot, never the public source.
        let lock = fs::read(fixture.0.join("ios/Package.resolved")).unwrap();
        fs::write(
            package.join("Package.resolved"),
            serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(&lock).unwrap())
                .unwrap(),
        )
        .unwrap();
        verify_resolution(&fixture.0.join("ios"), &package).unwrap();
        assert_eq!(
            fs::read(fixture.0.join("ios/Package.resolved")).unwrap(),
            lock
        );
        fs::write(package.join("Sources/Removed.swift"), b"stale").unwrap();
        fs::write(fixture.0.join("ios/Sources/Plugin.swift"), b"new source").unwrap();
        assert_eq!(fixture.stage().unwrap(), package);
        assert_eq!(fs::read(package.join("Package.resolved")).unwrap(), lock);
        assert_eq!(
            fs::read(package.join("Sources/Plugin.swift")).unwrap(),
            b"new source"
        );
        assert!(!package.join("Sources/Removed.swift").exists());
    }

    #[test]
    fn missing_malformed_changed_or_extra_resolved_pins_are_rejected() {
        let fixture = Fixture::new();
        let package = fixture.stage().unwrap();
        for content in [
            b"invalid".as_slice(),
            b"{\"pins\":[],\"version\":2}",
            b"{\"pins\":[{\"revision\":\"fixed\"},{\"revision\":\"extra\"}],\"version\":2}",
        ] {
            fs::write(package.join("Package.resolved"), content).unwrap();
            assert!(verify_resolution(&fixture.0.join("ios"), &package).is_err());
        }
        fs::remove_file(package.join("Package.resolved")).unwrap();
        assert!(verify_resolution(&fixture.0.join("ios"), &package).is_err());
    }

    #[test]
    fn known_swiftpm_envelopes_preserve_every_pin_and_reject_changed_identity() {
        let fixture = Fixture::new();
        let package = fixture.stage().unwrap();
        let original: serde_json::Value =
            serde_json::from_slice(include_bytes!("ios/Package.resolved")).unwrap();
        let pin = &original["pins"][0];
        let legacy = serde_json::json!({"version":1,"object":{"pins":[{
            "package":"SwiftRs", "repositoryURL":pin["location"], "state":{
                "branch":null, "revision":pin["state"]["revision"], "version":pin["state"]["version"]}}]}});
        let modern =
            serde_json::json!({"version":3,"pins":original["pins"],"originHash":"a".repeat(64)});
        for value in [&legacy, &modern] {
            fs::write(
                package.join("Package.resolved"),
                serde_json::to_vec(value).unwrap(),
            )
            .unwrap();
            verify_resolution(&fixture.0.join("ios"), &package).unwrap();
        }
        for changed in [
            "identity",
            "kind",
            "location",
            "revision",
            "version",
            "schema",
            "originHash",
        ] {
            let mut value = modern.clone();
            match changed {
                "revision" | "version" => value["pins"][0]["state"][changed] = "changed".into(),
                "schema" => value["version"] = 4.into(),
                "originHash" => value[changed] = "short".into(),
                _ => value["pins"][0][changed] = "changed".into(),
            }
            fs::write(
                package.join("Package.resolved"),
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
            assert!(
                verify_resolution(&fixture.0.join("ios"), &package).is_err(),
                "{changed}"
            );
        }
    }

    #[test]
    fn overlap_and_input_budgets_fail_before_staging() {
        let fixture = Fixture::new();
        let source = fixture.0.join("ios");
        assert!(stage(&source, &source).is_err());
        fs::write(
            source.join("Sources/TooLarge.swift"),
            vec![0; MAX_BYTES as usize + 1],
        )
        .unwrap();
        assert!(fixture.stage().is_err());
        assert!(!fixture.0.join("out/polaris-ios-package").exists());
        fs::remove_file(source.join("Sources/TooLarge.swift")).unwrap();
        for i in 0..MAX_ENTRIES {
            fs::write(source.join("Sources").join(format!("{i}.swift")), b"").unwrap();
        }
        assert!(fixture.stage().is_err());
        assert!(!fixture.0.join("out/polaris-ios-package").exists());
    }

    #[cfg(unix)]
    #[test]
    fn redirected_input_output_or_resolver_never_touches_external_files() {
        use std::os::unix::fs::symlink;
        for place in ["input", "output", "resolver", "sources"] {
            let fixture = Fixture::new();
            let outside = fixture.0.join("outside");
            fs::write(&outside, b"must remain exact").unwrap();
            match place {
                "input" => symlink(&outside, fixture.0.join("ios/Sources/Redirect.swift")).unwrap(),
                "output" => symlink(&outside, fixture.0.join("out/polaris-ios-package")).unwrap(),
                "resolver" => {
                    let package = fixture.stage().unwrap();
                    fs::remove_file(package.join("Package.resolved")).unwrap();
                    symlink(&outside, package.join("Package.resolved")).unwrap();
                }
                _ => {
                    let package = fixture.stage().unwrap();
                    symlink(&outside, package.join("Sources/Redirect.swift")).unwrap();
                }
            }
            assert!(fixture.stage().is_err(), "{place}");
            assert_eq!(fs::read(outside).unwrap(), b"must remain exact");
        }
    }
}
