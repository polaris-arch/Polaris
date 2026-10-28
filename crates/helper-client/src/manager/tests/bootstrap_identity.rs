//! Ordinary-user native primitives only: never run the administrator bootstrap.
use super::*;

#[test]
fn bootstrap_identity_render_is_platform_specific() {
    let mac =
        super::super::install_unix::sealed_bootstrap(Path::new("/inert"), &"a".repeat(64), true);
    let linux =
        super::super::install_unix::sealed_bootstrap(Path::new("/inert"), &"a".repeat(64), false);
    assert!(mac.contains("BOOT_FD_ID=$(/usr/bin/stat -f '%d:%i' <&8)"));
    assert!(!mac.contains("stat -f '%d:%i' /dev/fd/8"));
    assert!(linux.contains("BOOT_FD_ID=$(/usr/bin/stat -Lc '%d:%i' /dev/fd/8) || exit 1"));
    assert!(linux.contains("echo HELPER_BOOTSTRAP_IDENTITY_CHANGED >&2"));
    for marker in [
        "FD_QUERY_FAILED",
        "PATH_QUERY_FAILED",
        "FD_IDENTITY_CHANGED",
        "CLEANUP_IDENTITY_CHANGED",
    ] {
        assert!(mac.contains(&format!("HELPER_BOOTSTRAP_{marker}")));
        assert!(!linux.contains(&format!("HELPER_BOOTSTRAP_{marker}")));
    }
}

#[cfg(unix)]
#[test]
fn native_bootstrap_descriptor_identity() {
    use std::process::Command;
    // This harmless script is data for the hash and ordinary pathname execution.
    const HASH: &str = "306c6ca7407560340797866e077e053627ad409277d1b9da58106fce4cf717cb";
    let mac = cfg!(target_os = "macos");
    let rendered =
        super::super::install_unix::sealed_bootstrap(Path::new("/never-executed"), HASH, mac);
    let start = rendered.find("\nexec 8<\"$BOOT/helper\"\n").unwrap() + 1;
    let fragment = &rendered[start..];
    assert!(fragment.starts_with("exec 8<"));
    assert!(!fragment.contains("trap "));
    assert!(!fragment.contains("chown"));
    assert!(!fragment.contains("mktemp"));
    let (open_hash, identities) = fragment.split_once("BOOT_FD_ID=").unwrap();
    let identities = format!("BOOT_FD_ID={identities}");
    let query = identities.lines().next().unwrap();
    let guard = rendered
        .lines()
        .find(|s| s.starts_with("[ -f \"$BOOT/helper\" ]"))
        .unwrap();
    let cleanup = rendered
        .split_once("\nclose_bootstrap() {\n")
        .unwrap()
        .1
        .split_once("\n}\n")
        .unwrap()
        .0;
    assert!(cleanup.starts_with("  exec 8<&-"));
    assert!(!cleanup.contains("trap "));
    assert_eq!(cleanup.matches("/bin/rm -rf").count(), 1);
    let path_stat = if mac {
        "/usr/bin/stat -f '%d:%i'"
    } else {
        "/usr/bin/stat -Lc '%d:%i'"
    };
    for case in [
        "normal",
        "replaced",
        "symlink",
        "hash",
        "closed",
        "missing",
        "cleanup-replaced",
        "early-failure",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let boot = temp.path().join("owned");
        std::fs::create_dir(&boot).unwrap();
        write_executable_stand_in(&boot.join("helper"), b"#!/bin/sh\nexit 0\n");
        let mut script = format!(
            "set -eu\nset -o pipefail\nBOOT={}\n",
            shell_quote(&boot.to_string_lossy())
        );
        script.push_str(&format!(
            "BOOT_ID=$({path_stat} \"$BOOT\")\nclose_bootstrap() {{\n{cleanup}\n}}\n"
        ));
        match case {
            "symlink" => {
                script.push_str("/bin/mv \"$BOOT/helper\" \"$BOOT/original\"\n/bin/ln -s original \"$BOOT/helper\"\n");
                script.push_str(guard);
            }
            "hash" => {
                script.push_str("printf changed >\"$BOOT/helper\"\n");
                script.push_str(fragment);
            }
            "replaced" => {
                script.push_str(open_hash);
                script.push_str("/bin/cp \"$BOOT/helper\" \"$BOOT/new\"\n/bin/mv \"$BOOT/new\" \"$BOOT/helper\"\n");
                script.push_str(&identities);
            }
            "closed" => {
                script.push_str(open_hash);
                script.push_str("exec 8<&-\n");
                script.push_str(&identities);
            }
            "missing" => {
                script.push_str(open_hash);
                script.push_str("/bin/rm \"$BOOT/helper\"\n");
                script.push_str(&identities);
            }
            "early-failure" => {
                script.push_str(fragment);
                script.push_str("trap close_bootstrap EXIT\nexit 7\n");
            }
            "cleanup-replaced" => {
                script.push_str(fragment);
                script.push_str(
                    "/bin/mv \"$BOOT\" \"$BOOT.previous\"\n/bin/mkdir \"$BOOT\"\nclose_bootstrap\n",
                );
            }
            _ => {
                script.push_str(guard);
                script.push('\n');
                script.push_str(fragment);
                if mac {
                    // Darwin shasum's /dev/fd reopen shares offset: hash consumed it.
                    script.push_str("if IFS= read -r -u 8 line; then exit 21; fi\n");
                }
                script.push_str(query);
                script.push('\n');
                script.push_str(query);
                script.push('\n');
                script.push_str("[ \"$BOOT_FD_ID\" = \"$BOOT_FILE_ID\" ]\n");
                if mac {
                    script.push_str("if IFS= read -r -u 8 line; then exit 22; fi\n");
                    // Mechanism observation only: old devfs stat need not differ on every OS.
                    script.push_str("/usr/bin/stat -f '%d:%i' /dev/fd/8 >/dev/null\n");
                }
                script.push_str("/bin/sh \"$BOOT/helper\"\nclose_bootstrap\n[ ! -e \"$BOOT\" ]\nif (: <&8) 2>/dev/null; then exit 23; fi\n");
            }
        }
        let output = Command::new("/bin/bash")
            .arg("-c")
            .arg(&script)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            case == "normal",
            "case={case}, status={:?}",
            output.status
        );
        if case == "early-failure" {
            assert_eq!(output.status.code(), Some(7));
            assert!(!boot.exists());
        }
        if case == "cleanup-replaced" {
            assert!(boot.is_dir());
            assert!(temp.path().join("owned.previous/helper").is_file());
        }
        if mac {
            let expected = match case {
                "replaced" => Some("HELPER_BOOTSTRAP_FD_IDENTITY_CHANGED"),
                "closed" => Some("HELPER_BOOTSTRAP_FD_QUERY_FAILED"),
                "missing" => Some("HELPER_BOOTSTRAP_PATH_QUERY_FAILED"),
                "cleanup-replaced" => Some("HELPER_BOOTSTRAP_CLEANUP_IDENTITY_CHANGED"),
                "hash" => Some("HELPER_BUNDLE_MISMATCH"),
                _ => None,
            };
            if let Some(expected) = expected {
                assert!(
                    String::from_utf8_lossy(&output.stderr)
                        .lines()
                        .any(|s| s == expected),
                    "case={case}"
                );
            }
        }
    }
}
