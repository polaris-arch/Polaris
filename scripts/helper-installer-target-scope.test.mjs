import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
const read = (path) => readFileSync(new URL(`../crates/helper/${path}`, import.meta.url), 'utf8');
const desktop = '#[cfg(any(target_os = "linux", target_os = "macos"))]';
test('nix-backed installer module and API match desktop dependency scope', () => {
  assert.ok(read('src/lib.rs').includes(`${desktop}\npub mod protected_install_path;`));
  for (const name of ['snapshot_registration', 'restore_registration']) {
    assert.ok(read('src/user_control.rs').includes(`${desktop}\npub fn ${name}(`));
    assert.ok(read('src/user_control.rs').includes(`${desktop}\nfn ${name}_with_owner(`));
  }
  const manifest = read('Cargo.toml');
  for (const os of ['linux', 'macos']) assert.ok(manifest.includes(`[target.'cfg(target_os = "${os}")'.dependencies]`));
  assert.ok(!manifest.includes(`[target.'cfg(unix)'.dependencies]`), 'do not broaden desktop dependency permissions to mobile');
});
test('installer dispatch is desktop-only and unsupported commands fail closed', () => {
  const cli = read('src/core_cli.rs');
  for (const verbs of ['"sync-install-path" | "validate-install-path"', '"snapshot-user" | "restore-user"']) {
    assert.ok(cli.includes(`${desktop}\n    if matches!(\n        args.first().map(String::as_str),\n        Some(${verbs})`));
  }
  assert.ok(cli.includes('eprintln!("unknown helper subcommand");\n            return Some(std::process::ExitCode::FAILURE);'));
  assert.ok(read('src/core_cli/tests/mod.rs').includes('fn unsupported_installer_verbs_never_fall_through_to_daemon_mode()'));
});
