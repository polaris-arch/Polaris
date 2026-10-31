//! Administrator scripts share byte sealing and upgrade admission. Platform
//! service operations remain explicit so rollback restores the original service.
use super::*;

/// Root-private, byte-sealed utility. Reused by maintenance without trusting
/// installed product directories or executing the installed helper.
pub(super) fn sealed_bootstrap(source: &Path, helper_sha256: &str, mac: bool) -> String {
    format!(
        r#"[ "$(/usr/bin/id -u)" = 0 ] || exit 1
BOOT=$(/usr/bin/mktemp -d {temporary}/polaris-helper-bootstrap.XXXXXXXX)
[ -d "$BOOT" ] && [ ! -L "$BOOT" ] && [ "$({stat_owner} "$BOOT")" = 0 ] || exit 1
BOOT_ID=$({stat_identity} "$BOOT") || exit 1
[ -n "$BOOT_ID" ] || exit 1
close_bootstrap() {{
  exec 8<&- || return 1
  if [ -d "$BOOT" ] && [ ! -L "$BOOT" ] && [ "$({stat_identity} "$BOOT")" = "$BOOT_ID" ]; then
    /bin/rm -rf "$BOOT" || return 1
  else echo {cleanup_identity_error} >&2; return 1; fi
}}
{bootstrap_trap}
{clear_directory_acl}/bin/chmod 0700 "$BOOT"
[ "$({stat_identity} "$BOOT")" = "$BOOT_ID" ] || exit 1
for OBJECT in "$BOOT"/* "$BOOT"/.[!.]* "$BOOT"/..?*; do
  if [ -e "$OBJECT" ] || [ -L "$OBJECT" ]; then echo HELPER_BOOTSTRAP_NOT_EMPTY >&2; exit 1; fi
done
(set -C; : > "$BOOT/helper")
[ -f "$BOOT/helper" ] && [ ! -L "$BOOT/helper" ] || exit 1
/bin/cp {source} "$BOOT/helper"
{clear_helper_acl}{chown} root:{group} "$BOOT/helper"
/bin/chmod 0755 "$BOOT/helper"
exec 8<"$BOOT/helper"
[ "$({hash_command} /dev/fd/8 | /usr/bin/awk '{{print $1}}')" = {hash} ] || {{ echo HELPER_BUNDLE_MISMATCH >&2; exit 1; }}
BOOT_FD_ID=$({fd_identity}) || {fd_query_failure}
BOOT_FILE_ID=$({stat_identity} "$BOOT/helper") || {path_query_failure}
[ -n "$BOOT_FD_ID" ] && [ -n "$BOOT_FILE_ID" ] && [ "$BOOT_FD_ID" = "$BOOT_FILE_ID" ] || {{ echo {fd_identity_error} >&2; exit 1; }}
"#,
        bootstrap_trap = if mac {
            "trap close_bootstrap EXIT"
        } else {
            r#"bootstrap_cleanup() {
  rc=$?
  trap - EXIT HUP INT TERM
  FAILED_STAGE=${INSTALL_STAGE-preparation}
  if [ "$rc" -eq 0 ]; then FAILED_STAGE=cleanup; fi
  diagnostic_failure "$rc" "$FAILED_STAGE" recovery-unconfirmed || :
  RECOVERY=cleanup-returned
  if ! close_bootstrap; then
    RECOVERY=recovery-unconfirmed
    [ "$rc" -ne 0 ] || rc=1
  fi
  diagnostic_failure "$rc" "$FAILED_STAGE" "$RECOVERY" || :
  exit "$rc"
}
trap bootstrap_cleanup EXIT"#
        },
        source = shell_quote(&source.to_string_lossy()),
        hash = shell_quote(helper_sha256),
        temporary = if mac { "/private/tmp" } else { "/tmp" },
        stat_owner = if mac {
            "/usr/bin/stat -f '%u'"
        } else {
            "/usr/bin/stat -c '%u'"
        },
        stat_identity = if mac {
            "/usr/bin/stat -f '%d:%i'"
        } else {
            "/usr/bin/stat -Lc '%d:%i'"
        },
        fd_identity = if mac {
            // Darwin /dev/fd is a devfs vnode: pathname stat is NOT fstat.
            // No filename makes Apple stat use fstat(stdin). Redirection duplicates
            // the held descriptor without reading or changing its shared offset.
            "/usr/bin/stat -f '%d:%i' <&8"
        } else {
            "/usr/bin/stat -Lc '%d:%i' /dev/fd/8"
        },
        fd_query_failure = if mac {
            "{ echo HELPER_BOOTSTRAP_FD_QUERY_FAILED >&2; exit 1; }"
        } else {
            "exit 1"
        },
        path_query_failure = if mac {
            "{ echo HELPER_BOOTSTRAP_PATH_QUERY_FAILED >&2; exit 1; }"
        } else {
            "exit 1"
        },
        fd_identity_error = if mac {
            "HELPER_BOOTSTRAP_FD_IDENTITY_CHANGED"
        } else {
            "HELPER_BOOTSTRAP_IDENTITY_CHANGED"
        },
        cleanup_identity_error = if mac {
            "HELPER_BOOTSTRAP_CLEANUP_IDENTITY_CHANGED"
        } else {
            "HELPER_BOOTSTRAP_IDENTITY_CHANGED"
        },
        clear_directory_acl = if mac { "/bin/chmod -N \"$BOOT\"\n" } else { "" },
        clear_helper_acl = if mac {
            "/bin/chmod -N \"$BOOT/helper\"\n"
        } else {
            ""
        },
        chown = if mac {
            "/usr/sbin/chown"
        } else {
            "/usr/bin/chown"
        },
        group = if mac { "wheel" } else { "root" },
        hash_command = if mac {
            "/usr/bin/shasum -a 256"
        } else {
            "/usr/bin/sha256sum"
        },
    )
}

/// Validate each protected ancestor without normalizing existing objects.
pub(super) fn secure_directory_functions(mac: bool) -> String {
    format!(
        r#"owner_mode() {{ {owner_mode}; }}
secure_dir() {{
  # Existing installations are evidence, not material to normalize into trust.
  # Validate each ancestor; only a missing child beneath that parent is created.
  if [ "$1" != / ]; then secure_dir "$(dirname "$1")"; fi
  if [ -L "$1" ]; then echo HELPER_INSTALL_DIRECTORY_INVALID >&2; exit 1; fi
  if [ ! -e "$1" ]; then
    mkdir "$1"
    chown -h root:"$GROUP" "$1"
    chmod 0755 "$1"
  fi
  [ -d "$1" ] || {{ echo HELPER_INSTALL_DIRECTORY_INVALID >&2; exit 1; }}
  SECURITY=$(owner_mode "$1")
  [ "${{SECURITY%%:*}}" = 0 ] || {{ echo HELPER_INSTALL_DIRECTORY_OWNER_INVALID >&2; exit 1; }}
  MODE=${{SECURITY#*:}}
  case "$MODE" in ''|*[!0-7]*) echo HELPER_INSTALL_DIRECTORY_MODE_INVALID >&2; exit 1;; esac
{writable_failure}
{native_directory_check}}}
"#,
        owner_mode = if mac {
            "stat -f '%u:%Lp' \"$1\""
        } else {
            "stat -c '%u:%a' \"$1\""
        },
        writable_failure = if mac {
            "  [ \"$((0$MODE & 022))\" -eq 0 ] || { echo HELPER_INSTALL_DIRECTORY_WRITABLE >&2; exit 1; }"
        } else {
            "  [ \"$((0$MODE & 022))\" -eq 0 ] || {\n    if [ \"$1\" = /usr/local/lib/polaris ] && [ \"$MODE\" = 775 ]; then echo HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY >&2; fi\n    echo HELPER_INSTALL_DIRECTORY_WRITABLE >&2; exit 1;\n  }"
        },
        native_directory_check = if mac {
            "  \"$BOOT/helper\" validate-install-path --path \"$1\"\n"
        } else {
            ""
        },
    )
}

pub(super) const LEASE_OPEN: &str = r#"[ ! -e "$SUPPORT/helper-install.lock" ] && [ ! -L "$SUPPORT/helper-install.lock" ] || { echo LEGACY_HELPER_INSTALL_RECOVERY_REQUIRED >&2; exit 75; }
LOCK="$SUPPORT/helper-install.lease"
if [ ! -e "$LOCK" ]; then (umask 077; set -C; : > "$LOCK") || [ -f "$LOCK" ] || exit 75; fi
[ -f "$LOCK" ] && [ ! -L "$LOCK" ] || { echo HELPER_INSTALL_LOCK_INVALID >&2; exit 1; }
exec 9>>"$LOCK"
"#;

pub(super) const SET_PHASE: &str = r#"set_phase() {
  printf '%s\n' "$1" > "$TXDIR/transaction-phase.next" &&
    admin sync-install-path --path "$TXDIR/transaction-phase.next" &&
    mv -f "$TXDIR/transaction-phase.next" "$TXDIR/transaction-phase" &&
    admin sync-install-path --path "$TXDIR"
}
"#;

fn transaction(
    paths: &InstallPaths,
    params: &InstallParams,
    token: &str,
    mac: bool,
    migration: bool,
) -> String {
    let support = if mac {
        MAC_SUPPORT_DIR
    } else {
        LINUX_STATE_DIR
    };
    let group = if mac { "wheel" } else { "root" };
    let descriptor = paths.descriptor.as_ref().expect("Unix service descriptor");
    let program = paths.binary.to_string_lossy();
    let core = paths.core_dir.to_string_lossy();
    let service = if mac {
        SERVICE_LABEL
    } else {
        LINUX_SERVICE_NAME
    };
    // Trust the OS-managed utility paths and private temporary namespace, not
    // any existing product directory. Clear ACLs only on these new objects.
    let bootstrap = if mac {
        sealed_bootstrap(&params.src_binary, &params.helper_sha256, true)
    } else {
        "BOOT=''\n".to_owned()
    };
    let migration_helpers = r#"migration_archive() {
  RETIRED="$SUPPORT/helper-retired.${TXDIR##*.}"
  [ ! -e "$RETIRED" ] && [ ! -L "$RETIRED" ] || return 1
  mv "$TXDIR" "$RETIRED" || return 1
  TXDIR="$RETIRED"
  admin sync-install-path --path "$SUPPORT" || return 1
  TXDIR=''
}
migration_activate() {
  # Already committed: activation may only move forward, never restore old code.
  "$BOOT/helper" validate-install-path --path "$TXDIR/descriptor" || return 1
  "$BOOT/helper" validate-install-path --path "$TXDIR/migration-core-id" || return 1
  CORE_ID=$(cat "$TXDIR/migration-core-id") || return 1
  admin migration-candidate --journal "$TXDIR" --id "$CORE_ID" --phase committed || return 1
  set_phase activation-pending || return 1
  install -o root -g "$GROUP" -m 0644 "$TXDIR/descriptor" "$DESCRIPTOR.new.$$" || return 1
  mv -f "$DESCRIPTOR.new.$$" "$DESCRIPTOR" || return 1
  admin sync-install-path --path "$DESCRIPTOR" || return 1
  admin sync-install-path --path "$(dirname "$DESCRIPTOR")" || return 1
  launchctl enable "system/$SERVICE" || return 1
  # Existing candidates make bootstrap fail; only the identity/readiness probe
  # below decides whether that failure can be accepted.
  launchctl bootstrap system "$DESCRIPTOR" || :
  admin migration-candidate --journal "$TXDIR" --id "$CORE_ID" --phase active || return 1
  admin cancel-upgrade --support "$SUPPORT" || return 1
  admin migration-candidate --journal "$TXDIR" --id "$CORE_ID" --phase active || return 1
  set_phase completed || return 1
  rm -f "$TXDIR/recovery-required"
  migration_archive
}
"#;
    let migration_rollback = r#"  if [ "$MIGRATION" -eq 1 ]; then
    PHASE=$(cat "$TXDIR/transaction-phase") || PHASE=unknown
    case "$PHASE" in
      committed-closed|activation-pending|completed|unknown)
        touch "$TXDIR/recovery-required"
        echo HELPER_MIGRATION_ACTIVATION_PENDING >&2
        exec 9>&- || cleanup_failure
        close_bootstrap || :
        exit 1;;
    esac
    if [ -e "$TXDIR/migration-stop.started" ] && [ ! -f "$TXDIR/migration-stop.complete" ]; then
      touch "$TXDIR/recovery-required"
      echo HELPER_MIGRATION_STOP_UNCONFIRMED >&2
      exec 9>&- || cleanup_failure
      close_bootstrap || :
      exit 1
    fi
    if [ -f "$TXDIR/migration-stop.complete" ]; then
      if ! admin migration-rollback --journal "$TXDIR"; then
        touch "$TXDIR/recovery-required"
        echo HELPER_ROLLBACK_STOP_UNCONFIRMED >&2
        exec 9>&- || cleanup_failure
        close_bootstrap || :
        exit 1
      fi
      restore_original() {
        if [ -f "$TXDIR/$1.original" ]; then
          if [ -e "$2" ]; then mv "$2" "$TXDIR/$1.candidate" || return 1; fi
          mv "$TXDIR/$1.original" "$2" || return 1
        elif [ "$3" -eq 0 ] && [ -e "$2" ]; then
          mv "$2" "$TXDIR/$1.candidate" || return 1
        fi
        if [ -f "$2" ]; then admin sync-install-path --path "$2" || return 1; fi
        admin sync-install-path --path "$(dirname "$2")"
      }
      restore_original helper "$DEST" "$HAD_HELPER" || OK=0
      restore_original descriptor "$DESCRIPTOR" "$HAD_DESCRIPTOR" || OK=0
      restore_original record "$RECORD" "$HAD_RECORD" || OK=0
      restore_registration || OK=0
      if [ "$OK" -eq 1 ]; then
        case "$(cat "$TXDIR/migration-original-disabled")" in
          false|true) launchctl enable "system/$SERVICE" || OK=0;;
          *) OK=0;;
        esac
      fi
      if [ "$OK" -eq 1 ] && [ "$WAS_ACTIVE" -eq 1 ]; then
        launchctl bootstrap system "$DESCRIPTOR" || OK=0
      fi
      if [ "$OK" -eq 1 ] && [ "$(cat "$TXDIR/migration-original-disabled")" = true ]; then
        launchctl disable "system/$SERVICE" || OK=0
      fi
      if [ "$OK" -eq 1 ]; then
        set_phase rolled-back || OK=0
        migration_archive || OK=0
      fi
      if [ "$OK" -ne 1 ]; then touch "$TXDIR/recovery-required"; fi
    fi
    if [ -n "$TXDIR" ] && [ ! -e "$TXDIR/recovery-required" ]; then rm -rf "$TXDIR" || cleanup_failure; fi
    exec 9>&- || cleanup_failure
    close_bootstrap || :
    exit "$rc"
  fi
"#;
    let header = format!(
        r#"#!/bin/sh
set -eu
{diagnostics}PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 077
DEST={dest}
DESCRIPTOR={descriptor}
SUPPORT={support}
COREDIR={core}
SERVICE={service}
GROUP={group}
MIGRATION={migration_flag}
{bootstrap}admin() {{
  if [ -n "$BOOT" ]; then "$BOOT/helper" "$@"; else "$TXDIR/helper" "$@"; fi
}}
{secure_directories}
INSTALL_STAGE=directories
secure_dir {parent}
secure_dir "$SUPPORT"
secure_dir "$COREDIR"
{legacy_mask_hint}{lease_open}
LEASED=0
TXDIR=''
ADMITTED=0
MUTATED=0
HAD_HELPER=0
HAD_DESCRIPTOR=0
HAD_RECORD=0
REGISTRATION_CHANGED=0
KEEP_HELPER=0
WAS_ACTIVE=0
REQUIRE_CLOSURE={require_closure}
WAS_ENABLED=0
RECORD="$SUPPORT/helper-installed.json"
{migration_helpers}cleanup_failure() {{
  FAILURE_RC=$?
  if [ "${{POLARIS_DIAGNOSTICS-0}}" = 1 ]; then diagnostic_failure "$FAILURE_RC" "$FAILED_STAGE" recovery-unconfirmed || :; fi
  exit "$FAILURE_RC"
}}
cleanup() {{
  rc=$?
  FAILED_STAGE=${{INSTALL_STAGE-preparation}}
  if [ "$rc" -eq 0 ]; then FAILED_STAGE=cleanup; fi
  if [ "${{POLARIS_DIAGNOSTICS-0}}" = 1 ]; then diagnostic_failure "$rc" "$FAILED_STAGE" recovery-unconfirmed || :; fi
  RECOVERY=cleanup-returned
  trap - EXIT HUP INT TERM
  if [ "$ADMITTED" -eq 1 ]; then admin cancel-upgrade --support "$SUPPORT" >/dev/null 2>&1 || {{ touch "$TXDIR/recovery-required"; rc=1; RECOVERY=recovery-unconfirmed; }}; fi
  # Cleanup is part of the lease transaction. Closing fd 9 releases the kernel
  # lease only after the journal has disappeared; the helper lives in TXDIR.
  if [ -n "$TXDIR" ] && [ ! -e "$TXDIR/recovery-required" ]; then rm -rf "$TXDIR" || cleanup_failure; fi
  exec 9>&- || cleanup_failure
  if [ -n "$BOOT" ]; then close_bootstrap || {{ rc=1; RECOVERY=recovery-unconfirmed; }}; fi
  if [ "${{POLARIS_DIAGNOSTICS-0}}" = 1 ]; then diagnostic_failure "$rc" "$FAILED_STAGE" "$RECOVERY" || :; fi
  exit "$rc"
}}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
INSTALL_STAGE=staging
TXDIR=$(mktemp -d "$SUPPORT/helper-admission.XXXXXX")
chmod 0700 "$TXDIR"
cp {source} "$TXDIR/helper"
chown root:"$GROUP" "$TXDIR/helper"; chmod 0755 "$TXDIR/helper"
cp {manifest} "$TXDIR/core-manifest.json"
chown root:"$GROUP" "$TXDIR/core-manifest.json"; chmod 0600 "$TXDIR/core-manifest.json"
hash_file() {{ {hash_command}; }}
owner_mode() {{ {owner_mode}; }}
[ "$(hash_file "$TXDIR/helper")" = {helper_hash} ] || {{ echo HELPER_BUNDLE_MISMATCH >&2; exit 1; }}
[ "$(hash_file "$TXDIR/core-manifest.json")" = {manifest_hash} ] || {{ echo CORE_BUNDLE_MISMATCH >&2; exit 1; }}
INSTALL_STAGE=lease
admin lease-acquire --lock "$LOCK" --fd 9 || {{ echo HELPER_INSTALL_TRANSACTION_BUSY >&2; exit 75; }}
LEASED=1
INSTALL_STAGE=journal
# Admission bytes live outside the recovery namespace until the lease is held.
# Publish the initialized journal by same-directory rename, never mkdir there.
printf '%s\n' polaris-helper-upgrade-v1 > "$TXDIR/transaction-kind"
printf '%s' "$DEST" > "$TXDIR/transaction-destination"
printf '%s\n' staging > "$TXDIR/transaction-phase"
{set_phase}
for OLD in "$SUPPORT"/helper-upgrade.*; do
  [ -e "$OLD" ] || [ -L "$OLD" ] || continue
  [ -d "$OLD" ] && [ ! -L "$OLD" ] && [ "$(owner_mode "$OLD")" = '0:700' ] || {{ echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }}
  admin validate-install-path --path "$OLD"
  for OBJECT in "$OLD/transaction-kind" "$OLD/transaction-destination" "$OLD/transaction-phase" "$OLD/helper"; do
    [ -f "$OBJECT" ] && [ ! -L "$OBJECT" ] || {{ echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }}
    admin validate-install-path --path "$OBJECT"
  done
  [ "$(cat "$OLD/transaction-kind")" = polaris-helper-upgrade-v1 ] && printf '%s' "$DEST" | cmp -s - "$OLD/transaction-destination" || {{ echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }}
  OLD_PHASE=$(cat "$OLD/transaction-phase")
  case "$OLD_PHASE" in
    committed-closed|activation-pending)
      [ "$MIGRATION" -eq 1 ] && [ -f "$OLD/migration-preserve" ] || {{ echo HELPER_MIGRATION_ACTIVATION_PENDING >&2; exit 75; }}
      ADMISSION_TXDIR="$TXDIR"
      TXDIR="$OLD"
      touch "$TXDIR/recovery-required"
      migration_activate || {{ echo HELPER_MIGRATION_ACTIVATION_PENDING >&2; exit 1; }}
      rm -rf "$ADMISSION_TXDIR"
      echo polaris-helper-install-ok
      exit 0;;
  esac
  [ ! -e "$OLD/recovery-required" ] && [ ! -L "$OLD/recovery-required" ] || {{ echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }}
  case "$(cat "$OLD/transaction-phase")" in
    staging) rm -rf "$OLD";;
    admission)
      # These phases precede any service mutation. Cancel a possibly accepted
      # upgrade reservation with the current sealed utility before retirement.
      admin cancel-upgrade --support "$SUPPORT" >/dev/null 2>&1 || {{ echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }}
      rm -rf "$OLD";;
    completed|rolled-back)
      if [ -f "$OLD/migration-preserve" ]; then
        RETIRED="$SUPPORT/helper-retired.${{OLD##*.}}"
        [ ! -e "$RETIRED" ] && [ ! -L "$RETIRED" ] || exit 75
        mv "$OLD" "$RETIRED"
        admin sync-install-path --path "$SUPPORT"
      else rm -rf "$OLD"; fi;;
    *) echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75;;
  esac
done
PUBLISHED="$SUPPORT/helper-upgrade.${{TXDIR##*.}}"
[ ! -e "$PUBLISHED" ] && [ ! -L "$PUBLISHED" ] || exit 75
for OBJECT in helper core-manifest.json transaction-kind transaction-destination transaction-phase; do
  admin sync-install-path --path "$TXDIR/$OBJECT"
done
admin sync-install-path --path "$TXDIR"
mv "$TXDIR" "$PUBLISHED"
TXDIR="$PUBLISHED"
admin sync-install-path --path "$SUPPORT"
if [ "$MIGRATION" -eq 1 ]; then
  printf '%s\n' preserve-original-inodes > "$TXDIR/migration-preserve"
  printf '%s\n' {core_id} > "$TXDIR/migration-core-id"
  admin sync-install-path --path "$TXDIR/migration-preserve"
  admin sync-install-path --path "$TXDIR/migration-core-id"
  admin sync-install-path --path "$TXDIR"
fi
printf '%s' {token} > "$TXDIR/user-token"
chmod 0600 "$TXDIR/user-token"
INSTALL_STAGE=core-install
admin core-install --root "$COREDIR" --source {payload} --manifest "$TXDIR/core-manifest.json" --source-owner {identity}{preserve}
snapshot_registration() {{
  admin snapshot-user --support "$SUPPORT" --identity {identity} --backup "$TXDIR/registration.rollback"
}}
register_user() {{
  set_phase registration
  REGISTRATION_CHANGED=1
  INSTALL_STAGE=registration
  admin register-user --support "$SUPPORT" --identity {identity} --confdir {config} --token-file "$TXDIR/user-token"
}}
restore_registration() {{
  if [ "$REGISTRATION_CHANGED" -eq 1 ]; then
    admin restore-user --support "$SUPPORT" --identity {identity} --backup "$TXDIR/registration.rollback" || return 1
    REGISTRATION_CHANGED=0
  fi
}}
INSTALL_STAGE=policy
POLICY=$(admin helper-policy --binary "$DEST" --metadata "$RECORD")
case "$POLICY" in
  'helper-policy retain') KEEP_HELPER=1;;
  'helper-policy replace'|'helper-policy replace-legacy') ;;
  *) echo HELPER_INSTALLED_RECEIPT_INVALID >&2; exit 1;;
esac
for OBJECT in "$DEST" "$DESCRIPTOR" "$RECORD"; do
  if [ -L "$OBJECT" ] || {{ [ -e "$OBJECT" ] && [ ! -f "$OBJECT" ]; }}; then echo "invalid service object: $OBJECT" >&2; exit 1; fi
done
if [ -f "$DEST" ]; then cp "$DEST" "$TXDIR/helper.rollback"; HAD_HELPER=1; fi
if [ -f "$DESCRIPTOR" ]; then cp "$DESCRIPTOR" "$TXDIR/descriptor.rollback"; HAD_DESCRIPTOR=1; fi
if [ -f "$RECORD" ]; then cp "$RECORD" "$TXDIR/record.rollback"; HAD_RECORD=1; fi
for OBJECT in helper.rollback descriptor.rollback record.rollback; do
  if [ -f "$TXDIR/$OBJECT" ]; then admin sync-install-path --path "$TXDIR/$OBJECT"; fi
done
admin sync-install-path --path "$TXDIR"
rollback() {{
  rc=$?
  FAILED_STAGE=${{INSTALL_STAGE-preparation}}
  if [ "$rc" -eq 0 ]; then FAILED_STAGE=cleanup; fi
  if [ "${{POLARIS_DIAGNOSTICS-0}}" = 1 ]; then diagnostic_failure "$rc" "$FAILED_STAGE" recovery-unconfirmed || :; fi
  trap - EXIT HUP INT TERM
  OK=1
{migration_rollback}  if [ "$MUTATED" -eq 1 ]; then
    if ! stop_service; then
      # Never overwrite/restart while the replacement service may still own
      # native connections. Preserve all recovery material for a later action.
      touch "$TXDIR/recovery-required"
      echo HELPER_ROLLBACK_STOP_UNCONFIRMED >&2
      exec 9>&- || cleanup_failure
      if [ -n "$BOOT" ]; then close_bootstrap || {{ rc=1; OK=0; }}; fi
      exit 1
    fi
    if [ "$KEEP_HELPER" -eq 0 ]; then
    if [ "$HAD_HELPER" -eq 1 ]; then
      install -o root -g "$GROUP" -m 0755 "$TXDIR/helper.rollback" "$DEST.new.$$" && mv -f "$DEST.new.$$" "$DEST" || OK=0
    else rm -f "$DEST" || OK=0; fi
    fi
    if [ "$HAD_DESCRIPTOR" -eq 1 ]; then
      install -o root -g "$GROUP" -m 0644 "$TXDIR/descriptor.rollback" "$DESCRIPTOR.new.$$" && mv -f "$DESCRIPTOR.new.$$" "$DESCRIPTOR" || OK=0
    else rm -f "$DESCRIPTOR" || OK=0; fi
    if [ "$KEEP_HELPER" -eq 0 ]; then
    if [ "$HAD_RECORD" -eq 1 ]; then
      install -o root -g "$GROUP" -m 0644 "$TXDIR/record.rollback" "$RECORD.new.$$" && mv -f "$RECORD.new.$$" "$RECORD" || OK=0
    else rm -f "$RECORD" || OK=0; fi
    fi
    restore_registration || OK=0
    if [ "$OK" -eq 1 ]; then restore_service || OK=0; fi
  elif [ "$ADMITTED" -eq 1 ]; then
    admin cancel-upgrade --support "$SUPPORT" >/dev/null 2>&1 || OK=0
  fi
  restore_registration || OK=0
  rm -f "$DEST.new.$$" "$DESCRIPTOR.new.$$" "$RECORD.new.$$"
  if [ "$OK" -eq 1 ]; then rm -rf "$TXDIR" || cleanup_failure; else touch "$TXDIR/recovery-required"; echo "helper rollback incomplete; recovery transaction retained: $TXDIR" >&2; fi
  exec 9>&- || cleanup_failure
  if [ -n "$BOOT" ]; then close_bootstrap || {{ rc=1; OK=0; }}; fi
  if [ "${{POLARIS_DIAGNOSTICS-0}}" = 1 ]; then
    RECOVERY=recovery-unconfirmed
    if [ "$OK" -eq 1 ]; then RECOVERY=rollback-returned; fi
    diagnostic_failure "$rc" "$FAILED_STAGE" "$RECOVERY" || :
  fi
  exit "$rc"
}}
"#,
        migration_flag = if migration { 1 } else { 0 },
        migration_helpers = if migration { migration_helpers } else { "" },
        migration_rollback = if migration { migration_rollback } else { "" },
        diagnostics = if mac {
            ""
        } else {
            linux_service_stop::DIAGNOSTICS
        },
        bootstrap = bootstrap,
        require_closure = u8::from(!mac),
        secure_directories = secure_directory_functions(mac),
        legacy_mask_hint = if mac {
            ""
        } else {
            "if [ -L \"$DESCRIPTOR\" ] && [ \"$(readlink -- \"$DESCRIPTOR\")\" = /dev/null ]; then\n  echo HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY >&2\n  exit 1\nfi\nfor LEGACY_PENDING in \"$SUPPORT\"/helper-upgrade.*; do\n  if [ -e \"$LEGACY_PENDING/legacy-transaction\" ]; then\n    echo HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY >&2\n    exit 1\n  fi\ndone\n"
        },
        lease_open = LEASE_OPEN,
        set_phase = SET_PHASE,
        dest = shell_quote(&program),
        descriptor = shell_quote(&descriptor.to_string_lossy()),
        support = shell_quote(support),
        core = shell_quote(&core),
        service = shell_quote(service),
        group = group,
        parent = shell_quote(
            &paths
                .binary
                .parent()
                .expect("helper parent")
                .to_string_lossy()
        ),
        source = shell_quote(&params.src_binary.to_string_lossy()),
        manifest = shell_quote(&params.manifest_path.to_string_lossy()),
        helper_hash = shell_quote(&params.helper_sha256),
        manifest_hash = shell_quote(&params.manifest_sha256),
        hash_command = if mac {
            "shasum -a 256 \"$1\" | awk '{print $1}'"
        } else {
            "sha256sum \"$1\" | awk '{print $1}'"
        },
        owner_mode = if mac {
            "stat -f '%u:%Lp' \"$1\""
        } else {
            "stat -c '%u:%a' \"$1\""
        },
        token = shell_quote(token),
        payload = shell_quote(
            &params
                .bundled_core
                .parent()
                .expect("staged core parent")
                .to_string_lossy()
        ),
        core_id = shell_quote(params.core_id.as_str()),
        identity = shell_quote(&params.user_identity.key()),
        preserve = if migration {
            " --preserve-existing"
        } else {
            ""
        },
        config = shell_quote(&params.conf_dir.to_string_lossy()),
    );
    let authorization = if mac {
        String::new()
    } else {
        format!(
            r#"AUTHFILE={authfile}
HAD_AUTH=0
AUTH_SNAPSHOTTED=0
AUTH_MODE=0600
snapshot_authorization() {{
  if [ "$AUTH_SNAPSHOTTED" -eq 1 ]; then return; fi
  if [ -L "$AUTHFILE" ] || {{ [ -e "$AUTHFILE" ] && [ ! -f "$AUTHFILE" ]; }}; then echo invalid-authfile >&2; exit 1; fi
  if [ -f "$AUTHFILE" ]; then
    [ "$(stat -c %u "$AUTHFILE")" = 0 ] || {{ echo invalid-authfile-owner >&2; exit 1; }}
    AUTH_MODE=$(stat -c %a "$AUTHFILE")
    case "$AUTH_MODE" in ''|*[!0-7]*) echo invalid-authfile-mode >&2; exit 1;; esac
    cp "$AUTHFILE" "$TXDIR/auth.rollback"
    chmod 0600 "$TXDIR/auth.rollback"
    HAD_AUTH=1
  fi
  AUTH_SNAPSHOTTED=1
}}
merge_authorization() {{
  snapshot_authorization
  AUTHNEW="$TXDIR/auth.next"
  if [ "$HAD_AUTH" -eq 1 ]; then cp "$TXDIR/auth.rollback" "$AUTHNEW"; else (umask 077; touch "$AUTHNEW"); fi
  grep -qxF {uid} "$AUTHNEW" || printf '%s\n' {uid} >> "$AUTHNEW"
  chown root:root "$AUTHNEW"; chmod 0600 "$AUTHNEW"
  mv -f "$AUTHNEW" "$AUTHFILE"
}}
restore_authorization() {{
  if [ "$AUTH_SNAPSHOTTED" -eq 0 ]; then return; fi
  if [ "$HAD_AUTH" -eq 1 ]; then
    install -o root -g root -m "$AUTH_MODE" "$TXDIR/auth.rollback" "$AUTHFILE.new.$$" && mv -f "$AUTHFILE.new.$$" "$AUTHFILE" || return 1
  else rm -f "$AUTHFILE" || return 1; fi
}}
"#,
            authfile = shell_quote(LINUX_AUTH_FILE),
            uid = shell_quote(&params.uid.to_string())
        )
    };
    // Merely preparing a new CoreID/registration must not extend authorization
    // on an old daemon which can still accept retired content RPCs.
    let mut header = header.replace("POLICY=$(", &format!("{authorization}POLICY=$("));
    if !mac {
        header = header.replace(
            "  restore_registration || OK=0\n  rm -f",
            "  restore_authorization || OK=0\n  restore_registration || OK=0\n  rm -f",
        );
        header = header.replace(
            "    restore_registration || OK=0",
            "    restore_authorization || OK=0\n    restore_registration || OK=0",
        );
    }
    let service_proof = if mac {
        ""
    } else {
        linux_service_stop::FUNCTIONS
    };
    let platform = if mac {
        let plist = render_mac_plist(&program, &core, support, false);
        let closed = if migration {
            format!("cat > \"$TXDIR/descriptor.closed\" <<'POLARIS_CLOSED_DESCRIPTOR_EOF'\n{}\nPOLARIS_CLOSED_DESCRIPTOR_EOF\n",
                render_mac_plist(&program, &core, support, true))
        } else {
            String::new()
        };
        format!(
            r#"stop_service() {{
  if launchctl print "system/$SERVICE" >/dev/null 2>&1; then launchctl bootout "system/$SERVICE" || return 1; fi
  WAIT=50
  while launchctl print "system/$SERVICE" >/dev/null 2>&1; do
    WAIT=$((WAIT - 1)); [ "$WAIT" -gt 0 ] || return 1; sleep 0.1
  done
}}
restore_service() {{
  if [ "$WAS_ACTIVE" -eq 1 ]; then launchctl enable "system/$SERVICE" && launchctl bootstrap system "$DESCRIPTOR"; fi
}}
if launchctl print "system/$SERVICE" >/dev/null 2>&1; then WAS_ACTIVE=1; fi
cat > "$TXDIR/descriptor" <<'POLARIS_DESCRIPTOR_EOF'
{plist}
POLARIS_DESCRIPTOR_EOF
{closed}"#
        )
    } else {
        let unit = build_linux_unit(paths);
        format!(
            r#"stop_service() {{
  linux_validate_service || return 1
  INSTALL_STAGE=service-stop
  if [ "$LOAD" = loaded ]; then systemctl --system stop "$SERVICE" || return 1; fi
  linux_service_closed
}}
restore_service() {{
  linux_cgroup_view || return 1
  systemctl daemon-reload || return 1
  linux_validate_service || return 1
  if [ "$WAS_ENABLED" -eq 1 ]; then systemctl enable "$SERVICE" || return 1;
  elif [ "$HAD_DESCRIPTOR" -eq 1 ]; then systemctl disable "$SERVICE" || return 1; fi
  if [ "$WAS_ACTIVE" -eq 1 ]; then systemctl restart "$SERVICE" || return 1; fi
}}
INSTALL_STAGE=service-snapshot
INITIAL_STATE=$(linux_service_property ActiveState) || {{ echo HELPER_SERVICE_STATE_UNKNOWN >&2; exit 1; }}
case "$INITIAL_STATE" in
  active) WAS_ACTIVE=1;;
  inactive|failed) ;;
  *) echo HELPER_SERVICE_STATE_UNKNOWN >&2; exit 1;;
esac
if systemctl is-enabled --quiet "$SERVICE"; then WAS_ENABLED=1; fi
cat > "$TXDIR/descriptor" <<'POLARIS_DESCRIPTOR_EOF'
{unit}POLARIS_DESCRIPTOR_EOF
"#
        )
    };
    let admission = if migration {
        r#"trap rollback EXIT
[ "$HAD_HELPER" -eq 1 ] && [ "$HAD_DESCRIPTOR" -eq 1 ] && [ "$WAS_ACTIVE" -eq 1 ] || { echo HELPER_MIGRATION_NO_ACTIVE_INSTALLATION >&2; exit 1; }
KEEP_HELPER=0
set_phase legacy-stop-started
admin migration-stop --journal "$TXDIR"
set_phase legacy-stop-proved
admin migration-recheck --journal "$TXDIR"
set_phase service-mutation
MUTATED=1
# Preserve the actual original inodes. Copies above are never described as these.
mv "$DEST" "$TXDIR/helper.original"
mv "$DESCRIPTOR" "$TXDIR/descriptor.original"
if [ "$HAD_RECORD" -eq 1 ]; then mv "$RECORD" "$TXDIR/record.original"; fi
admin sync-install-path --path "$TXDIR"
admin sync-install-path --path "$(dirname "$DEST")"
admin sync-install-path --path "$(dirname "$DESCRIPTOR")"
admin sync-install-path --path "$SUPPORT"
register_user
install -o root -g "$GROUP" -m 0755 "$TXDIR/helper" "$DEST.new.$$"
mv "$DEST.new.$$" "$DEST"
install -o root -g "$GROUP" -m 0644 "$TXDIR/descriptor.closed" "$DESCRIPTOR.new.$$"
mv "$DESCRIPTOR.new.$$" "$DESCRIPTOR"
admin helper-record --binary "$DEST" --metadata "$RECORD"
"#
    } else {
        r#"trap rollback EXIT
if [ "$WAS_ACTIVE" -eq 1 ]; then
  set_phase admission
  if ! admin prepare-upgrade --support "$SUPPORT"; then
    echo HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY >&2
    exit 75
  fi
  ADMITTED=1
fi
set_phase service-mutation
MUTATED=1
if [ "$WAS_ACTIVE" -eq 1 ] || [ "$REQUIRE_CLOSURE" -eq 1 ]; then stop_service; fi
register_user
if [ "$KEEP_HELPER" -eq 0 ]; then
  install -o root -g "$GROUP" -m 0755 "$TXDIR/helper" "$DEST.new.$$"
  mv -f "$DEST.new.$$" "$DEST"
fi
install -o root -g "$GROUP" -m 0644 "$TXDIR/descriptor" "$DESCRIPTOR.new.$$"
mv -f "$DESCRIPTOR.new.$$" "$DESCRIPTOR"
if [ "$KEEP_HELPER" -eq 0 ]; then admin helper-record --binary "$DEST" --metadata "$RECORD"; fi
"#
    };
    let takeover = if migration {
        r#"for OBJECT in "$DEST" "$DESCRIPTOR" "$RECORD" "$TXDIR/descriptor" "$TXDIR/descriptor.closed" "$TXDIR/migration-core-id"; do
  admin sync-install-path --path "$OBJECT"
done
admin sync-install-path --path "$(dirname "$DEST")"
admin sync-install-path --path "$(dirname "$DESCRIPTOR")"
admin sync-install-path --path "$SUPPORT"
set_phase candidate-starting
launchctl enable "system/$SERVICE"
launchctl bootstrap system "$DESCRIPTOR"
CORE_ID=$(cat "$TXDIR/migration-core-id")
admin migration-candidate --journal "$TXDIR" --id "$CORE_ID" --phase closed
set_phase committed-closed
migration_activate
trap cleanup EXIT
echo polaris-helper-install-ok
exit 0
"#
    } else if mac {
        r#"launchctl enable "system/$SERVICE"
launchctl bootstrap system "$DESCRIPTOR"
launchctl print "system/$SERVICE" >/dev/null
"#
    } else {
        r#"INSTALL_STAGE=service-start
systemctl daemon-reload
systemctl enable "$SERVICE"
systemctl restart "$SERVICE"
INSTALL_STAGE=takeover
TAKEOVER_OK=0
WAIT=50
while [ "$WAIT" -gt 0 ]; do
  if systemctl is-active --quiet "$SERVICE"; then
    PID=$(systemctl show -p MainPID --value "$SERVICE")
    case "$PID" in ''|0|*[!0-9]*) ;; *) if cmp -s "/proc/$PID/exe" "$DEST"; then TAKEOVER_OK=1; break; fi;; esac
  fi
  WAIT=$((WAIT - 1)); sleep 0.1
done
[ "$TAKEOVER_OK" -eq 1 ] || { echo HELPER_TAKEOVER_TIMEOUT >&2; exit 1; }
"#
    };
    let grant = if mac { "" } else { "merge_authorization\n" };
    let retain = if migration {
        String::new()
    } else {
        format!(
            r#"if [ "$KEEP_HELPER" -eq 1 ] && [ "$WAS_ACTIVE" -eq 1 ] && admin retain-probe --support "$SUPPORT" --id {id}; then
trap rollback EXIT
register_user
{grant}set_phase completed
trap cleanup EXIT
echo polaris-helper-core-only-ok
exit 0
fi
"#,
            id = shell_quote(params.core_id.as_str())
        )
    };
    format!("{header}{service_proof}{platform}snapshot_registration\n{retain}{admission}{grant}{takeover}ADMITTED=0\nset_phase completed\ntrap cleanup EXIT\necho polaris-helper-install-ok\n")
}

pub(super) fn build_mac_install_script(
    paths: &InstallPaths,
    params: &InstallParams,
    token: &str,
) -> String {
    transaction(paths, params, token, true, false)
}
pub(super) fn build_linux_install_script(
    paths: &InstallPaths,
    params: &InstallParams,
    token: &str,
) -> String {
    transaction(paths, params, token, false, false)
}

/// Explicit, user-confirmed stop-and-upgrade. Ordinary repair never selects it.
pub(super) fn build_mac_migration_script(
    paths: &InstallPaths,
    params: &InstallParams,
    token: &str,
) -> String {
    transaction(paths, params, token, true, true)
}

/// One fixed native dispatcher owns legacy root mutations. Ordinary and
/// compatible-retain flows continue into their complete existing producer.
pub(super) fn linux_legacy_wrapper(
    source: &Path,
    helper_sha256: &str,
    operation: &str,
    config: &Path,
    original: Option<&[String; 10]>,
    native_arguments: &str,
    ordinary: &str,
) -> String {
    let baseline = original.map(|values| {
        values
            .iter()
            .map(|value| shell_quote(value))
            .collect::<Vec<_>>()
            .join(" ")
    });
    let baseline = baseline.map_or_else(
        || ": > \"$BOOT/original-proxy\"\n".to_owned(),
        |values| format!("printf '%s\\n' {values} > \"$BOOT/original-proxy\"\n"),
    );
    format!(
        r#"#!/bin/sh
set -eu
{diagnostics}PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 077
{bootstrap}admin() {{ "$BOOT/helper" "$@"; }}
GROUP=root
SUPPORT='/var/lib/polaris'
{secure}secure_dir "$SUPPORT"
secure_dir '/etc/systemd/system'
{lease}{baseline}CLASS=$(admin linux-legacy-classify --operation {operation})
case "$CLASS" in
  legacy)
    admin linux-legacy-{operation} --config {config} --original-file "$BOOT/original-proxy"{native_arguments}
    exec 9>&-
    exit 0;;
  ordinary)
    exec 9>&-
    close_bootstrap
    trap - EXIT HUP INT TERM;;
  *) echo LINUX_LEGACY_RECOVERY_REQUIRED >&2; exit 75;;
esac
{ordinary}"#,
        diagnostics = linux_service_stop::DIAGNOSTICS,
        bootstrap = sealed_bootstrap(source, helper_sha256, false),
        secure = secure_directory_functions(false),
        lease = LEASE_OPEN,
        config = shell_quote(&config.to_string_lossy()),
    )
}

pub(super) fn build_linux_legacy_install_script(
    paths: &InstallPaths,
    params: &InstallParams,
    token: &str,
) -> String {
    let original = build_linux_install_script(paths, params, token);
    let arguments = format!(" --source {} --manifest {} --helper-sha {} --manifest-sha {} --id {} --token-file \"$BOOT/user-token\"",
        shell_quote(&params.bundled_core.parent().expect("core stage parent").to_string_lossy()),
        shell_quote(&params.manifest_path.to_string_lossy()), shell_quote(&params.helper_sha256),
        shell_quote(&params.manifest_sha256), shell_quote(params.core_id.as_str()));
    let wrapper = linux_legacy_wrapper(
        &params.src_binary,
        &params.helper_sha256,
        "install",
        &params.conf_dir,
        params.linux_legacy_original_proxy.as_ref(),
        &arguments,
        &original,
    );
    // Token data remains in a root-private file, never in a helper argv.
    wrapper.replace(
        "CLASS=$(admin linux-legacy-classify",
        &format!(
            "printf '%s' {} > \"$BOOT/user-token\"\nCLASS=$(admin linux-legacy-classify",
            shell_quote(token)
        ),
    )
}
