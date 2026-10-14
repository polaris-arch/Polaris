//! Export the locked Tauri C entry points without changing registry sources.
//! Internal @_cdecl definitions are local/private in the native iOS archive.
//! Stage only Swift package inputs under the existing Cargo output slot; the
//! implementations and Package.swift/Package.resolved remain unchanged.
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_ENTRIES: usize = 256;
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const TAURI: &[&str] = &[
    "log_stdout",
    "register_plugin",
    "on_webview_created",
    "run_plugin_command",
];

fn reject() -> io::Error {
    io::Error::other("locked iOS C export staging input differs or exceeds its budget")
}

pub fn export_entries(source: &str, symbols: &[&str]) -> io::Result<String> {
    let mut result = source.to_owned();
    for symbol in symbols {
        let declaration = format!("@_cdecl(\"{symbol}\")\nfunc ");
        if result.matches(&declaration).count() != 1
            || result.matches(&format!("@_cdecl(\"{symbol}\")")).count() != 1
        {
            return Err(reject());
        }
        result = result.replace(
            &declaration,
            &format!("@_cdecl(\"{symbol}\")\npublic func "),
        );
    }
    Ok(result)
}

fn ordinary_directory(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(reject());
    }
    Ok(())
}

fn collect(
    path: &Path,
    entries: &mut usize,
    bytes: &mut u64,
    files: &mut Vec<(PathBuf, Vec<u8>)>,
) -> io::Result<()> {
    *entries += 1;
    if *entries > MAX_ENTRIES {
        return Err(reject());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            collect(&entry?.path(), entries, bytes, files)?;
        }
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
        if *bytes > MAX_BYTES || metadata.len() != content.len() as u64 {
            return Err(reject());
        }
        files.push((path.to_owned(), content));
    } else {
        return Err(reject());
    }
    Ok(())
}

fn inputs(
    source: &Path,
    entries: &mut usize,
    bytes: &mut u64,
) -> io::Result<Vec<(PathBuf, Vec<u8>)>> {
    ordinary_directory(source)?;
    ordinary_directory(&source.join("Sources"))?;
    if !fs::symlink_metadata(source.join("Package.swift"))?.is_file() {
        return Err(reject());
    }
    let mut files = Vec::new();
    for name in ["Package.swift", "Package.resolved", "Sources", "Tests"] {
        let path = source.join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => collect(&path, entries, bytes, &mut files)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    Ok(files)
}

fn adapt(
    files: &mut [(PathBuf, Vec<u8>)],
    source: &Path,
    file: &str,
    symbols: &[&str],
) -> io::Result<()> {
    let path = source.join(file);
    let (_, content) = files
        .iter_mut()
        .find(|(name, _)| *name == path)
        .ok_or_else(reject)?;
    let text = std::str::from_utf8(content).map_err(|_| reject())?;
    *content = export_entries(text, symbols)?.into_bytes();
    Ok(())
}

fn write_inputs(files: &[(PathBuf, Vec<u8>)], source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for (path, content) in files {
        let dest = destination.join(path.strip_prefix(source).map_err(|_| reject())?);
        fs::create_dir_all(dest.parent().ok_or_else(reject)?)?;
        fs::write(dest, content)?;
    }
    Ok(())
}

pub fn stage(name: &str, version: &str, source: &Path, out: &Path) -> io::Result<PathBuf> {
    let (expected, file, symbols): (&str, &str, &[&str]) = match name {
        "Tauri" => ("2.11.5", "Sources/Tauri/Tauri.swift", TAURI),
        "tauri-plugin-dialog" => (
            "2.7.2",
            "Sources/DialogPlugin.swift",
            &["init_plugin_dialog"],
        ),
        "tauri-plugin-notification" => (
            "2.3.3",
            "Sources/NotificationPlugin.swift",
            &["init_plugin_notification"],
        ),
        "tauri-plugin-shell" => ("2.3.5", "Sources/ShellPlugin.swift", &["init_plugin_shell"]),
        _ => return Ok(source.to_owned()),
    };
    if version != expected {
        return Err(reject());
    }
    ordinary_directory(source)?;
    ordinary_directory(out)?;
    let source = source.canonicalize()?;
    let out = out.canonicalize()?;
    if source.starts_with(&out) || out.starts_with(&source) {
        return Err(reject());
    }
    let (mut entries, mut bytes) = (0, 0);
    let mut package = inputs(&source, &mut entries, &mut bytes)?;
    adapt(&mut package, &source, file, symbols)?;
    // Plugin manifests resolve Tauri via ../.tauri/tauri-api. Preserve that
    // relative layout, adapting the same four entry points in this copy too.
    let dependency = source.parent().ok_or_else(reject)?.join(".tauri/tauri-api");
    let tauri = if name == "Tauri" {
        Vec::new()
    } else {
        let mut files = inputs(&dependency, &mut entries, &mut bytes)?;
        adapt(&mut files, &dependency, "Sources/Tauri/Tauri.swift", TAURI)?;
        files
    };
    let root = out.join("polaris-c-exports");
    match fs::symlink_metadata(&root) {
        Ok(_) => {
            ordinary_directory(&root)?;
            // Only the owned small staged source slot is restored, never the
            // Swift build products, Cargo cache or original source packages.
            collect(&root, &mut 0, &mut 0, &mut Vec::new())?;
            fs::remove_dir_all(&root)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let staged = root.join("ios");
    write_inputs(&package, &source, &staged)?;
    if !tauri.is_empty() {
        write_inputs(&tauri, &dependency, &root.join(".tauri/tauri-api"))?;
    }
    Ok(staged)
}
