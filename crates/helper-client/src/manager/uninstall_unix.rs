//! Explicit Unix maintenance uses the installer's sealed utility and stable
//! inherited lease. macOS retirement preserves evidence rather than restarting
//! or deleting an installation when closure cannot be proved.
use super::install_unix::{sealed_bootstrap, secure_directory_functions, LEASE_OPEN, SET_PHASE};
use super::{shell_quote, InstallPaths, MaintenanceUtility, LINUX_STATE_DIR, MAC_SUPPORT_DIR};

const COMMON_TRANSACTION: &str = r#"
TXDIR=''
TXDIR_ID=''
FINAL_STATE=unchanged
FINISHED=0
cleanup() {
  rc=$?
  trap - EXIT HUP INT TERM
  if [ "$FINISHED" -ne 1 ]; then
    # Only this invocation's positively unchanged Mac preflight is disposable.
    # Never infer no mutation from a shell existence test or an old receipt.
    if [ "$MAINTENANCE_MAC" -eq 1 ] && [ -n "$TXDIR" ] && [ -n "$TXDIR_ID" ] &&
       admin migration-uninstall-preflight-unchanged --journal "$TXDIR" >/dev/null 2>&1 &&
       [ "$(file_identity "$TXDIR")" = "$TXDIR_ID" ]; then
      if rm -rf "$TXDIR"; then
        TXDIR=''
        FINAL_STATE=unchanged
        admin sync-install-path --path "$SUPPORT" || rc=1
      fi
    fi
    if [ -n "$TXDIR" ]; then
      touch "$TXDIR/recovery-required" || :
      admin sync-install-path --path "$TXDIR/recovery-required" >/dev/null 2>&1 || :
      admin sync-install-path --path "$TXDIR" >/dev/null 2>&1 || :
    fi
    echo "HELPER_UNINSTALL_RECOVERY_REQUIRED state=$FINAL_STATE journal=$TXDIR" >&2
    [ "$rc" -ne 0 ] || rc=1
  fi
  # The parent directory and lease inode survive uninstall. Do not unlink,
  # rename or explicitly unlock it while this shell owns inherited fd 9.
  exec 9>&-
  close_bootstrap || rc=1
  if [ "$FINISHED" -eq 1 ] && [ "$rc" -eq 0 ]; then
    echo "polaris-helper-uninstall-state=$FINAL_STATE"
    if [ -n "$TXDIR" ]; then echo "polaris-helper-uninstall-journal=$TXDIR"; fi
    echo polaris-helper-uninstall-ok
  fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
admin lease-acquire --lock "$LOCK" --fd 9 || { echo HELPER_INSTALL_TRANSACTION_BUSY >&2; exit 75; }
# Old receipts are evidence only. This operation neither resumes nor deletes
# an interrupted install/uninstall, and never treats a prior stop as authority.
for OLD in "$SUPPORT"/helper-upgrade.*; do
  [ -e "$OLD" ] || [ -L "$OLD" ] || continue
  echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2
  exit 75
done
TXDIR=$(mktemp -d "$SUPPORT/helper-admission.XXXXXXXX")
chmod 0700 "$TXDIR"
# Match the existing migration guard's journal and receipt binding exactly.
# Intent and phases distinguish maintenance; there is no second stop protocol.
printf '%s\n' polaris-helper-upgrade-v1 > "$TXDIR/transaction-kind"
printf '%s' "$DEST" > "$TXDIR/transaction-destination"
printf '%s\n' uninstall-staging > "$TXDIR/transaction-phase"
printf '%s\n' uninstall > "$TXDIR/maintenance-operation"
cp "$BOOT/helper" "$TXDIR/helper"
chmod 0755 "$TXDIR/helper"
for OBJECT in transaction-kind transaction-destination transaction-phase maintenance-operation helper; do
  admin sync-install-path --path "$TXDIR/$OBJECT"
done
admin sync-install-path --path "$TXDIR"
PUBLISHED="$SUPPORT/helper-upgrade.${TXDIR##*.}"
[ ! -e "$PUBLISHED" ] && [ ! -L "$PUBLISHED" ] || exit 75
mv "$TXDIR" "$PUBLISHED"
TXDIR="$PUBLISHED"
admin sync-install-path --path "$SUPPORT"
"#;

const MAC_RETIREMENT: &str = r#"
TXDIR_ID=$(file_identity "$TXDIR")
[ -n "$TXDIR_ID" ] || exit 1
printf '%s\n' preserve-original-inodes > "$TXDIR/migration-preserve"
admin sync-install-path --path "$TXDIR/migration-preserve"
admin sync-install-path --path "$TXDIR"
# Capture names before stop; migration-stop binds the same actual objects and
# rejects cross-device renames before it changes launchd state.
for OBJECT in "$DEST" "$DESCRIPTOR" "$RECORD"; do
  if [ -L "$OBJECT" ] || { [ -e "$OBJECT" ] && [ ! -f "$OBJECT" ]; }; then
    echo HELPER_UNINSTALL_OBJECT_INVALID >&2; exit 1
  fi
done
HELPER_ID=$(file_identity "$DEST")
DESCRIPTOR_ID=$(file_identity "$DESCRIPTOR")
RECORD_ID=''
if [ -f "$RECORD" ]; then RECORD_ID=$(file_identity "$RECORD"); fi
set_phase uninstall-stop-started
FINAL_STATE=stop-unconfirmed
admin migration-stop --journal "$TXDIR"
set_phase uninstall-stop-proved
admin migration-recheck --journal "$TXDIR"
FINAL_STATE=stopped-preserved
set_phase uninstall-retirement
retire_original() {
  [ -f "$1" ] && [ ! -L "$1" ] && [ "$(file_identity "$1")" = "$3" ] || return 1
  [ ! -e "$TXDIR/$2.original" ] && [ ! -L "$TXDIR/$2.original" ] || return 1
  mv "$1" "$TXDIR/$2.original" || return 1
  [ "$(file_identity "$TXDIR/$2.original")" = "$3" ] || return 1
  admin sync-install-path --path "$TXDIR/$2.original" || return 1
  admin sync-install-path --path "$(dirname "$1")" || return 1
  admin sync-install-path --path "$TXDIR"
}
FINAL_STATE=retirement-incomplete
retire_original "$DEST" helper "$HELPER_ID"
retire_original "$DESCRIPTOR" descriptor "$DESCRIPTOR_ID"
if [ -n "$RECORD_ID" ]; then retire_original "$RECORD" record "$RECORD_ID"; fi
# No activation, rollback, pid signaling, core removal or support cleanup.
# Archive only this completed transaction, retaining every original inode and
# the bound stop receipts. Archived receipts never authorize a later action.
set_phase uninstall-completed
RETIRED="$SUPPORT/helper-retired.${TXDIR##*.}"
[ ! -e "$RETIRED" ] && [ ! -L "$RETIRED" ] || exit 75
mv "$TXDIR" "$RETIRED"
TXDIR="$RETIRED"
admin sync-install-path --path "$SUPPORT"
FINAL_STATE=stopped-retired-data-preserved
FINISHED=1
"#;

// Deliberately retain the existing Linux stop/proof order. The proof implementation
// is shared with installation and is independently maintained in linux_service_stop.
const LINUX_REMOVAL: &str = r#"
set_phase uninstall-stop-started
FINAL_STATE=stop-unconfirmed
linux_validate_service || { echo HELPER_UNINSTALL_SERVICE_STATE_UNKNOWN >&2; exit 1; }
if [ "$LOAD" = loaded ]; then systemctl --system disable --now "$SERVICE"; fi
linux_service_closed || { echo HELPER_UNINSTALL_SERVICE_NOT_STOPPED >&2; exit 1; }
set_phase uninstall-removal
FINAL_STATE=removal-incomplete
rm -f "$DESCRIPTOR"
rm -rf "$INSTALLDIR" "$RUNTIMEDIR"
# Keep the same lease pathname, inode and parent. Recovery journals are not
# general product state and may not be swept as part of ordinary uninstall.
for OBJECT in "$SUPPORT"/* "$SUPPORT"/.[!.]* "$SUPPORT"/..?*; do
  [ -e "$OBJECT" ] || [ -L "$OBJECT" ] || continue
  case "$OBJECT" in
    "$LOCK"|"$TXDIR"|"$SUPPORT"/helper-upgrade.*|"$SUPPORT"/helper-admission.*) continue;;
  esac
  rm -rf "$OBJECT"
done
systemctl daemon-reload
set_phase uninstall-completed
rm -rf "$TXDIR"
TXDIR=''
admin sync-install-path --path "$SUPPORT"
FINAL_STATE=stopped-removed-lease-preserved
FINISHED=1
"#;

fn maintenance_header(paths: &InstallPaths, utility: &MaintenanceUtility, mac: bool) -> String {
    let descriptor = paths.descriptor.as_ref().expect("Unix service descriptor");
    format!(
        r#"#!/bin/sh
set -eu
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 077
DEST={destination}
DESCRIPTOR={descriptor}
SUPPORT={support}
SERVICE={service}
GROUP={group}
MAINTENANCE_MAC={mac_flag}
RECORD="$SUPPORT/helper-installed.json"
{bootstrap}admin() {{ "$BOOT/helper" "$@"; }}
{secure_directories}secure_dir {helper_parent}
secure_dir {descriptor_parent}
secure_dir "$SUPPORT"
{lease_open}{set_phase}{transaction}"#,
        destination = shell_quote(&paths.binary.to_string_lossy()),
        descriptor = shell_quote(&descriptor.to_string_lossy()),
        support = shell_quote(if mac {
            MAC_SUPPORT_DIR
        } else {
            LINUX_STATE_DIR
        }),
        service = shell_quote(paths.service_label),
        group = if mac { "wheel" } else { "root" },
        mac_flag = u8::from(mac),
        bootstrap = sealed_bootstrap(&utility.src_binary, &utility.helper_sha256, mac),
        secure_directories = secure_directory_functions(mac),
        helper_parent = shell_quote(
            &paths
                .binary
                .parent()
                .expect("helper parent")
                .to_string_lossy()
        ),
        descriptor_parent = shell_quote(
            &descriptor
                .parent()
                .expect("descriptor parent")
                .to_string_lossy()
        ),
        lease_open = LEASE_OPEN,
        set_phase = SET_PHASE,
        transaction = COMMON_TRANSACTION,
    )
}

pub(super) fn build_mac_uninstall_script(
    paths: &InstallPaths,
    utility: &MaintenanceUtility,
) -> String {
    format!(
        "{}file_identity() {{ /usr/bin/stat -f '%d:%i' \"$1\"; }}\n{MAC_RETIREMENT}",
        maintenance_header(paths, utility, true),
    )
}

pub(super) fn build_linux_uninstall_script(
    paths: &InstallPaths,
    utility: &MaintenanceUtility,
) -> String {
    let install_dir = paths.binary.parent().expect("helper parent");
    let runtime_dir = paths.socket.parent().expect("socket parent");
    format!(
        "{}INSTALLDIR={}\nRUNTIMEDIR={}\n{}{LINUX_REMOVAL}",
        maintenance_header(paths, utility, false),
        shell_quote(&install_dir.to_string_lossy()),
        shell_quote(&runtime_dir.to_string_lossy()),
        super::linux_service_stop::FUNCTIONS,
    )
}

#[cfg(test)]
mod tests;
