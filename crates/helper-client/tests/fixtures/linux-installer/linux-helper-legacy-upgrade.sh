#!/bin/sh
set -eu

POLARIS_DIAGNOSTICS=1
INSTALL_STAGE=preparation
diagnostic_failure() {
  [ "$1" -ne 0 ] || return 0
  case "$2" in
    preparation|directories|staging|lease|journal|core-install|policy|service-snapshot|manager-identity|manager-property|pid1-cgroup|cgroup-mount|unit-config|service-stop|service-closure|registration|service-install|service-start|takeover|uninstall-remove|cleanup|rollback) STAGE=$2;;
    *) STAGE=unknown;;
  esac
  case "$3" in not-started|cleanup-returned|rollback-returned|recovery-unconfirmed) RECOVERY=$3;; *) RECOVERY=recovery-unconfirmed;; esac
  printf 'POLARIS_HELPER_FAILURE_V1:%s:%s\n' "$STAGE" "$RECOVERY" >&2
}
trap 'rc=$?; diagnostic_failure "$rc" "$INSTALL_STAGE" not-started || :; exit "$rc"' EXIT
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 077
[ "$(/usr/bin/id -u)" = 0 ] || exit 1
BOOT=$(/usr/bin/mktemp -d /tmp/polaris-helper-bootstrap.XXXXXXXX)
[ -d "$BOOT" ] && [ ! -L "$BOOT" ] && [ "$(/usr/bin/stat -c '%u' "$BOOT")" = 0 ] || exit 1
BOOT_ID=$(/usr/bin/stat -Lc '%d:%i' "$BOOT") || exit 1
[ -n "$BOOT_ID" ] || exit 1
close_bootstrap() {
  exec 8<&- || return 1
  if [ -d "$BOOT" ] && [ ! -L "$BOOT" ] && [ "$(/usr/bin/stat -Lc '%d:%i' "$BOOT")" = "$BOOT_ID" ]; then
    /bin/rm -rf "$BOOT" || return 1
  else echo HELPER_BOOTSTRAP_IDENTITY_CHANGED >&2; return 1; fi
}
bootstrap_cleanup() {
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
trap bootstrap_cleanup EXIT
/bin/chmod 0700 "$BOOT"
[ "$(/usr/bin/stat -Lc '%d:%i' "$BOOT")" = "$BOOT_ID" ] || exit 1
for OBJECT in "$BOOT"/* "$BOOT"/.[!.]* "$BOOT"/..?*; do
  if [ -e "$OBJECT" ] || [ -L "$OBJECT" ]; then echo HELPER_BOOTSTRAP_NOT_EMPTY >&2; exit 1; fi
done
(set -C; : > "$BOOT/helper")
[ -f "$BOOT/helper" ] && [ ! -L "$BOOT/helper" ] || exit 1
/bin/cp '/tmp/producer fixture/用户'\''s stage/polaris-helper' "$BOOT/helper"
/usr/bin/chown root:root "$BOOT/helper"
/bin/chmod 0755 "$BOOT/helper"
exec 8<"$BOOT/helper"
[ "$(/usr/bin/sha256sum /dev/fd/8 | /usr/bin/awk '{print $1}')" = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' ] || { echo HELPER_BUNDLE_MISMATCH >&2; exit 1; }
BOOT_FD_ID=$(/usr/bin/stat -Lc '%d:%i' /dev/fd/8) || exit 1
BOOT_FILE_ID=$(/usr/bin/stat -Lc '%d:%i' "$BOOT/helper") || exit 1
[ -n "$BOOT_FD_ID" ] && [ -n "$BOOT_FILE_ID" ] && [ "$BOOT_FD_ID" = "$BOOT_FILE_ID" ] || { echo HELPER_BOOTSTRAP_IDENTITY_CHANGED >&2; exit 1; }
admin() { "$BOOT/helper" "$@"; }
GROUP=root
SUPPORT='/var/lib/polaris'
owner_mode() { stat -c '%u:%a' "$1"; }
secure_dir() {
  # Existing installations are evidence, not material to normalize into trust.
  # Validate each ancestor; only a missing child beneath that parent is created.
  if [ "$1" != / ]; then secure_dir "$(dirname "$1")"; fi
  if [ -L "$1" ]; then echo HELPER_INSTALL_DIRECTORY_INVALID >&2; exit 1; fi
  if [ ! -e "$1" ]; then
    mkdir "$1"
    chown -h root:"$GROUP" "$1"
    chmod 0755 "$1"
  fi
  [ -d "$1" ] || { echo HELPER_INSTALL_DIRECTORY_INVALID >&2; exit 1; }
  SECURITY=$(owner_mode "$1")
  [ "${SECURITY%%:*}" = 0 ] || { echo HELPER_INSTALL_DIRECTORY_OWNER_INVALID >&2; exit 1; }
  MODE=${SECURITY#*:}
  case "$MODE" in ''|*[!0-7]*) echo HELPER_INSTALL_DIRECTORY_MODE_INVALID >&2; exit 1;; esac
  [ "$((0$MODE & 022))" -eq 0 ] || {
    if [ "$1" = /usr/local/lib/polaris ] && [ "$MODE" = 775 ]; then echo HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY >&2; fi
    echo HELPER_INSTALL_DIRECTORY_WRITABLE >&2; exit 1;
  }
}
secure_dir "$SUPPORT"
secure_dir '/etc/systemd/system'
[ ! -e "$SUPPORT/helper-install.lock" ] && [ ! -L "$SUPPORT/helper-install.lock" ] || { echo LEGACY_HELPER_INSTALL_RECOVERY_REQUIRED >&2; exit 75; }
LOCK="$SUPPORT/helper-install.lease"
if [ ! -e "$LOCK" ]; then (umask 077; set -C; : > "$LOCK") || [ -f "$LOCK" ] || exit 75; fi
[ -f "$LOCK" ] && [ ! -L "$LOCK" ] || { echo HELPER_INSTALL_LOCK_INVALID >&2; exit 1; }
exec 9>>"$LOCK"
printf '%s\n' ''\''127.0.0.1'\''' '3128' 'false' ''\'''\''' '0' ''\'''\''' '0' '['\''localhost'\'', '\''127.0.0.0/8'\'']' ''\''none'\''' '/home/producer-user' > "$BOOT/original-proxy"
printf '%s' 'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee' > "$BOOT/user-token"
CLASS=$(admin linux-legacy-classify --operation install)
case "$CLASS" in
  legacy)
    admin linux-legacy-install --config '/tmp/producer fixture/用户'\''s config' --original-file "$BOOT/original-proxy" --source '/tmp/producer fixture/用户'\''s stage' --manifest '/tmp/producer fixture/用户'\''s stage/core-manifest.json' --helper-sha 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' --manifest-sha 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' --id 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc' --token-file "$BOOT/user-token"
    exec 9>&-
    exit 0;;
  ordinary)
    exec 9>&-
    close_bootstrap
    trap - EXIT HUP INT TERM;;
  *) echo LINUX_LEGACY_RECOVERY_REQUIRED >&2; exit 75;;
esac
#!/bin/sh
set -eu

POLARIS_DIAGNOSTICS=1
INSTALL_STAGE=preparation
diagnostic_failure() {
  [ "$1" -ne 0 ] || return 0
  case "$2" in
    preparation|directories|staging|lease|journal|core-install|policy|service-snapshot|manager-identity|manager-property|pid1-cgroup|cgroup-mount|unit-config|service-stop|service-closure|registration|service-install|service-start|takeover|uninstall-remove|cleanup|rollback) STAGE=$2;;
    *) STAGE=unknown;;
  esac
  case "$3" in not-started|cleanup-returned|rollback-returned|recovery-unconfirmed) RECOVERY=$3;; *) RECOVERY=recovery-unconfirmed;; esac
  printf 'POLARIS_HELPER_FAILURE_V1:%s:%s\n' "$STAGE" "$RECOVERY" >&2
}
trap 'rc=$?; diagnostic_failure "$rc" "$INSTALL_STAGE" not-started || :; exit "$rc"' EXIT
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 077
DEST='/usr/local/lib/polaris/helper'
DESCRIPTOR='/etc/systemd/system/polaris-helper.service'
SUPPORT='/var/lib/polaris'
COREDIR='/usr/local/lib/polaris/core'
SERVICE='polaris-helper.service'
GROUP=root
MIGRATION=0
BOOT=''
admin() {
  if [ -n "$BOOT" ]; then "$BOOT/helper" "$@"; else "$TXDIR/helper" "$@"; fi
}
owner_mode() { stat -c '%u:%a' "$1"; }
secure_dir() {
  # Existing installations are evidence, not material to normalize into trust.
  # Validate each ancestor; only a missing child beneath that parent is created.
  if [ "$1" != / ]; then secure_dir "$(dirname "$1")"; fi
  if [ -L "$1" ]; then echo HELPER_INSTALL_DIRECTORY_INVALID >&2; exit 1; fi
  if [ ! -e "$1" ]; then
    mkdir "$1"
    chown -h root:"$GROUP" "$1"
    chmod 0755 "$1"
  fi
  [ -d "$1" ] || { echo HELPER_INSTALL_DIRECTORY_INVALID >&2; exit 1; }
  SECURITY=$(owner_mode "$1")
  [ "${SECURITY%%:*}" = 0 ] || { echo HELPER_INSTALL_DIRECTORY_OWNER_INVALID >&2; exit 1; }
  MODE=${SECURITY#*:}
  case "$MODE" in ''|*[!0-7]*) echo HELPER_INSTALL_DIRECTORY_MODE_INVALID >&2; exit 1;; esac
  [ "$((0$MODE & 022))" -eq 0 ] || {
    if [ "$1" = /usr/local/lib/polaris ] && [ "$MODE" = 775 ]; then echo HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY >&2; fi
    echo HELPER_INSTALL_DIRECTORY_WRITABLE >&2; exit 1;
  }
}

INSTALL_STAGE=directories
secure_dir '/usr/local/lib/polaris'
secure_dir "$SUPPORT"
secure_dir "$COREDIR"
if [ -L "$DESCRIPTOR" ] && [ "$(readlink -- "$DESCRIPTOR")" = /dev/null ]; then
  echo HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY >&2
  exit 1
fi
for LEGACY_PENDING in "$SUPPORT"/helper-upgrade.*; do
  if [ -e "$LEGACY_PENDING/legacy-transaction" ]; then
    echo HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY >&2
    exit 1
  fi
done
[ ! -e "$SUPPORT/helper-install.lock" ] && [ ! -L "$SUPPORT/helper-install.lock" ] || { echo LEGACY_HELPER_INSTALL_RECOVERY_REQUIRED >&2; exit 75; }
LOCK="$SUPPORT/helper-install.lease"
if [ ! -e "$LOCK" ]; then (umask 077; set -C; : > "$LOCK") || [ -f "$LOCK" ] || exit 75; fi
[ -f "$LOCK" ] && [ ! -L "$LOCK" ] || { echo HELPER_INSTALL_LOCK_INVALID >&2; exit 1; }
exec 9>>"$LOCK"

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
REQUIRE_CLOSURE=1
WAS_ENABLED=0
RECORD="$SUPPORT/helper-installed.json"
cleanup_failure() {
  FAILURE_RC=$?
  if [ "${POLARIS_DIAGNOSTICS-0}" = 1 ]; then diagnostic_failure "$FAILURE_RC" "$FAILED_STAGE" recovery-unconfirmed || :; fi
  exit "$FAILURE_RC"
}
cleanup() {
  rc=$?
  FAILED_STAGE=${INSTALL_STAGE-preparation}
  if [ "$rc" -eq 0 ]; then FAILED_STAGE=cleanup; fi
  if [ "${POLARIS_DIAGNOSTICS-0}" = 1 ]; then diagnostic_failure "$rc" "$FAILED_STAGE" recovery-unconfirmed || :; fi
  RECOVERY=cleanup-returned
  trap - EXIT HUP INT TERM
  if [ "$ADMITTED" -eq 1 ]; then admin cancel-upgrade --support "$SUPPORT" >/dev/null 2>&1 || { touch "$TXDIR/recovery-required"; rc=1; RECOVERY=recovery-unconfirmed; }; fi
  # Cleanup is part of the lease transaction. Closing fd 9 releases the kernel
  # lease only after the journal has disappeared; the helper lives in TXDIR.
  if [ -n "$TXDIR" ] && [ ! -e "$TXDIR/recovery-required" ]; then rm -rf "$TXDIR" || cleanup_failure; fi
  exec 9>&- || cleanup_failure
  if [ -n "$BOOT" ]; then close_bootstrap || { rc=1; RECOVERY=recovery-unconfirmed; }; fi
  if [ "${POLARIS_DIAGNOSTICS-0}" = 1 ]; then diagnostic_failure "$rc" "$FAILED_STAGE" "$RECOVERY" || :; fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
INSTALL_STAGE=staging
TXDIR=$(mktemp -d "$SUPPORT/helper-admission.XXXXXX")
chmod 0700 "$TXDIR"
cp '/tmp/producer fixture/用户'\''s stage/polaris-helper' "$TXDIR/helper"
chown root:"$GROUP" "$TXDIR/helper"; chmod 0755 "$TXDIR/helper"
cp '/tmp/producer fixture/用户'\''s stage/core-manifest.json' "$TXDIR/core-manifest.json"
chown root:"$GROUP" "$TXDIR/core-manifest.json"; chmod 0600 "$TXDIR/core-manifest.json"
hash_file() { sha256sum "$1" | awk '{print $1}'; }
owner_mode() { stat -c '%u:%a' "$1"; }
[ "$(hash_file "$TXDIR/helper")" = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' ] || { echo HELPER_BUNDLE_MISMATCH >&2; exit 1; }
[ "$(hash_file "$TXDIR/core-manifest.json")" = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' ] || { echo CORE_BUNDLE_MISMATCH >&2; exit 1; }
INSTALL_STAGE=lease
admin lease-acquire --lock "$LOCK" --fd 9 || { echo HELPER_INSTALL_TRANSACTION_BUSY >&2; exit 75; }
LEASED=1
INSTALL_STAGE=journal
# Admission bytes live outside the recovery namespace until the lease is held.
# Publish the initialized journal by same-directory rename, never mkdir there.
printf '%s\n' polaris-helper-upgrade-v1 > "$TXDIR/transaction-kind"
printf '%s' "$DEST" > "$TXDIR/transaction-destination"
printf '%s\n' staging > "$TXDIR/transaction-phase"
set_phase() {
  printf '%s\n' "$1" > "$TXDIR/transaction-phase.next" &&
    admin sync-install-path --path "$TXDIR/transaction-phase.next" &&
    mv -f "$TXDIR/transaction-phase.next" "$TXDIR/transaction-phase" &&
    admin sync-install-path --path "$TXDIR"
}

for OLD in "$SUPPORT"/helper-upgrade.*; do
  [ -e "$OLD" ] || [ -L "$OLD" ] || continue
  [ -d "$OLD" ] && [ ! -L "$OLD" ] && [ "$(owner_mode "$OLD")" = '0:700' ] || { echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }
  admin validate-install-path --path "$OLD"
  for OBJECT in "$OLD/transaction-kind" "$OLD/transaction-destination" "$OLD/transaction-phase" "$OLD/helper"; do
    [ -f "$OBJECT" ] && [ ! -L "$OBJECT" ] || { echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }
    admin validate-install-path --path "$OBJECT"
  done
  [ "$(cat "$OLD/transaction-kind")" = polaris-helper-upgrade-v1 ] && printf '%s' "$DEST" | cmp -s - "$OLD/transaction-destination" || { echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }
  OLD_PHASE=$(cat "$OLD/transaction-phase")
  case "$OLD_PHASE" in
    committed-closed|activation-pending)
      [ "$MIGRATION" -eq 1 ] && [ -f "$OLD/migration-preserve" ] || { echo HELPER_MIGRATION_ACTIVATION_PENDING >&2; exit 75; }
      ADMISSION_TXDIR="$TXDIR"
      TXDIR="$OLD"
      touch "$TXDIR/recovery-required"
      migration_activate || { echo HELPER_MIGRATION_ACTIVATION_PENDING >&2; exit 1; }
      rm -rf "$ADMISSION_TXDIR"
      echo polaris-helper-install-ok
      exit 0;;
  esac
  [ ! -e "$OLD/recovery-required" ] && [ ! -L "$OLD/recovery-required" ] || { echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }
  case "$(cat "$OLD/transaction-phase")" in
    staging) rm -rf "$OLD";;
    admission)
      # These phases precede any service mutation. Cancel a possibly accepted
      # upgrade reservation with the current sealed utility before retirement.
      admin cancel-upgrade --support "$SUPPORT" >/dev/null 2>&1 || { echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75; }
      rm -rf "$OLD";;
    completed|rolled-back)
      if [ -f "$OLD/migration-preserve" ]; then
        RETIRED="$SUPPORT/helper-retired.${OLD##*.}"
        [ ! -e "$RETIRED" ] && [ ! -L "$RETIRED" ] || exit 75
        mv "$OLD" "$RETIRED"
        admin sync-install-path --path "$SUPPORT"
      else rm -rf "$OLD"; fi;;
    *) echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2; exit 75;;
  esac
done
PUBLISHED="$SUPPORT/helper-upgrade.${TXDIR##*.}"
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
  printf '%s\n' 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc' > "$TXDIR/migration-core-id"
  admin sync-install-path --path "$TXDIR/migration-preserve"
  admin sync-install-path --path "$TXDIR/migration-core-id"
  admin sync-install-path --path "$TXDIR"
fi
printf '%s' 'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee' > "$TXDIR/user-token"
chmod 0600 "$TXDIR/user-token"
INSTALL_STAGE=core-install
admin core-install --root "$COREDIR" --source '/tmp/producer fixture/用户'\''s stage' --manifest "$TXDIR/core-manifest.json" --source-owner 'u1001'
snapshot_registration() {
  admin snapshot-user --support "$SUPPORT" --identity 'u1001' --backup "$TXDIR/registration.rollback"
}
register_user() {
  set_phase registration
  REGISTRATION_CHANGED=1
  INSTALL_STAGE=registration
  admin register-user --support "$SUPPORT" --identity 'u1001' --confdir '/tmp/producer fixture/用户'\''s config' --token-file "$TXDIR/user-token"
}
restore_registration() {
  if [ "$REGISTRATION_CHANGED" -eq 1 ]; then
    admin restore-user --support "$SUPPORT" --identity 'u1001' --backup "$TXDIR/registration.rollback" || return 1
    REGISTRATION_CHANGED=0
  fi
}
INSTALL_STAGE=policy
AUTHFILE='/var/lib/polaris/authorized-uids'
HAD_AUTH=0
AUTH_SNAPSHOTTED=0
AUTH_MODE=0600
snapshot_authorization() {
  if [ "$AUTH_SNAPSHOTTED" -eq 1 ]; then return; fi
  if [ -L "$AUTHFILE" ] || { [ -e "$AUTHFILE" ] && [ ! -f "$AUTHFILE" ]; }; then echo invalid-authfile >&2; exit 1; fi
  if [ -f "$AUTHFILE" ]; then
    [ "$(stat -c %u "$AUTHFILE")" = 0 ] || { echo invalid-authfile-owner >&2; exit 1; }
    AUTH_MODE=$(stat -c %a "$AUTHFILE")
    case "$AUTH_MODE" in ''|*[!0-7]*) echo invalid-authfile-mode >&2; exit 1;; esac
    cp "$AUTHFILE" "$TXDIR/auth.rollback"
    chmod 0600 "$TXDIR/auth.rollback"
    HAD_AUTH=1
  fi
  AUTH_SNAPSHOTTED=1
}
merge_authorization() {
  snapshot_authorization
  AUTHNEW="$TXDIR/auth.next"
  if [ "$HAD_AUTH" -eq 1 ]; then cp "$TXDIR/auth.rollback" "$AUTHNEW"; else (umask 077; touch "$AUTHNEW"); fi
  grep -qxF '1001' "$AUTHNEW" || printf '%s\n' '1001' >> "$AUTHNEW"
  chown root:root "$AUTHNEW"; chmod 0600 "$AUTHNEW"
  mv -f "$AUTHNEW" "$AUTHFILE"
}
restore_authorization() {
  if [ "$AUTH_SNAPSHOTTED" -eq 0 ]; then return; fi
  if [ "$HAD_AUTH" -eq 1 ]; then
    install -o root -g root -m "$AUTH_MODE" "$TXDIR/auth.rollback" "$AUTHFILE.new.$$" && mv -f "$AUTHFILE.new.$$" "$AUTHFILE" || return 1
  else rm -f "$AUTHFILE" || return 1; fi
}
POLICY=$(admin helper-policy --binary "$DEST" --metadata "$RECORD")
case "$POLICY" in
  'helper-policy retain') KEEP_HELPER=1;;
  'helper-policy replace'|'helper-policy replace-legacy') ;;
  *) echo HELPER_INSTALLED_RECEIPT_INVALID >&2; exit 1;;
esac
for OBJECT in "$DEST" "$DESCRIPTOR" "$RECORD"; do
  if [ -L "$OBJECT" ] || { [ -e "$OBJECT" ] && [ ! -f "$OBJECT" ]; }; then echo "invalid service object: $OBJECT" >&2; exit 1; fi
done
if [ -f "$DEST" ]; then cp "$DEST" "$TXDIR/helper.rollback"; HAD_HELPER=1; fi
if [ -f "$DESCRIPTOR" ]; then cp "$DESCRIPTOR" "$TXDIR/descriptor.rollback"; HAD_DESCRIPTOR=1; fi
if [ -f "$RECORD" ]; then cp "$RECORD" "$TXDIR/record.rollback"; HAD_RECORD=1; fi
for OBJECT in helper.rollback descriptor.rollback record.rollback; do
  if [ -f "$TXDIR/$OBJECT" ]; then admin sync-install-path --path "$TXDIR/$OBJECT"; fi
done
admin sync-install-path --path "$TXDIR"
rollback() {
  rc=$?
  FAILED_STAGE=${INSTALL_STAGE-preparation}
  if [ "$rc" -eq 0 ]; then FAILED_STAGE=cleanup; fi
  if [ "${POLARIS_DIAGNOSTICS-0}" = 1 ]; then diagnostic_failure "$rc" "$FAILED_STAGE" recovery-unconfirmed || :; fi
  trap - EXIT HUP INT TERM
  OK=1
  if [ "$MUTATED" -eq 1 ]; then
    if ! stop_service; then
      # Never overwrite/restart while the replacement service may still own
      # native connections. Preserve all recovery material for a later action.
      touch "$TXDIR/recovery-required"
      echo HELPER_ROLLBACK_STOP_UNCONFIRMED >&2
      exec 9>&- || cleanup_failure
      if [ -n "$BOOT" ]; then close_bootstrap || { rc=1; OK=0; }; fi
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
    restore_authorization || OK=0
    restore_registration || OK=0
    if [ "$OK" -eq 1 ]; then restore_service || OK=0; fi
  elif [ "$ADMITTED" -eq 1 ]; then
    admin cancel-upgrade --support "$SUPPORT" >/dev/null 2>&1 || OK=0
  fi
  restore_authorization || OK=0
  restore_registration || OK=0
  rm -f "$DEST.new.$$" "$DESCRIPTOR.new.$$" "$RECORD.new.$$"
  if [ "$OK" -eq 1 ]; then rm -rf "$TXDIR" || cleanup_failure; else touch "$TXDIR/recovery-required"; echo "helper rollback incomplete; recovery transaction retained: $TXDIR" >&2; fi
  exec 9>&- || cleanup_failure
  if [ -n "$BOOT" ]; then close_bootstrap || { rc=1; OK=0; }; fi
  if [ "${POLARIS_DIAGNOSTICS-0}" = 1 ]; then
    RECOVERY=recovery-unconfirmed
    if [ "$OK" -eq 1 ]; then RECOVERY=rollback-returned; fi
    diagnostic_failure "$rc" "$FAILED_STAGE" "$RECOVERY" || :
  fi
  exit "$rc"
}

linux_systemd_property() {
  PROPERTY=$1
  shift
  RECORD=$(systemctl --system show --all --property="$PROPERTY" "$@") || return 1
  # Empty values are valid for some properties, but a missing property is not.
  # Accept exactly one keyed record, never an omitted/duplicate/foreign key.
  case "$RECORD" in *'
'*) return 1;; esac
  case "$RECORD" in
    "$PROPERTY="*) printf '%s\n' "${RECORD#*=}";;
    *) return 1;;
  esac
}
linux_service_property() {
  linux_systemd_property "$1" "$SERVICE"
}
linux_cgroup_view() {
  INSTALL_STAGE=manager-identity
  # A cgroup2 filesystem can be a subtree/bind view. Anchor this view to the
  # system manager's namespaces, and require the mount's real hierarchy root.
  MANAGER_NAME=$(cat /proc/1/comm) || return 1
  [ "$MANAGER_NAME" = systemd ] || return 1
  MANAGER_OWNER=$(stat -c %u /proc/1) || return 1
  [ "$MANAGER_OWNER" = 0 ] || return 1
  for NAMESPACE in pid mnt cgroup; do
    OUR_NAMESPACE=$(readlink "/proc/self/ns/$NAMESPACE") || return 1
    MANAGER_NAMESPACE=$(readlink "/proc/1/ns/$NAMESPACE") || return 1
    [ -n "$OUR_NAMESPACE" ] && [ "$OUR_NAMESPACE" = "$MANAGER_NAMESPACE" ] || return 1
  done
  INSTALL_STAGE=manager-property
  MANAGER_GROUP=$(linux_systemd_property ControlGroup) || return 1
  # systemd stores the hierarchy root without its trailing slash, including
  # the root itself. An explicit empty property is not a missing record.
  case "$MANAGER_GROUP" in ''|/) ;; *) return 1;; esac
  INSTALL_STAGE=cgroup-mount
  CGROOT=/sys/fs/cgroup
  [ -d "$CGROOT" ] && [ ! -L "$CGROOT" ] || return 1
  CGTYPE=$(stat -f -c %T "$CGROOT") || return 1
  [ "$CGTYPE" = cgroup2fs ] || return 1
  MOUNTS=$(cat /proc/self/mountinfo) || return 1
  MATCHES=0
  while read -r MOUNT_ID PARENT_ID DEVICE ROOT MOUNTPOINT OPTIONS REST; do
    case "$MOUNTPOINT" in "$CGROOT"/*) return 1;; esac
    if [ "$MOUNTPOINT" = "$CGROOT" ]; then
      [ "$ROOT" = / ] || return 1
      case " $REST " in *" - cgroup2 "*) ;; *) return 1;; esac
      MATCHES=$((MATCHES + 1))
    fi
  done <<POLARIS_MOUNTINFO
$MOUNTS
POLARIS_MOUNTINFO
  [ "$MATCHES" -eq 1 ] || return 1
  INSTALL_STAGE=pid1-cgroup
  PID1_GROUP=$(cat /proc/1/cgroup) || return 1
  case "$PID1_GROUP" in
    '0::/') PID1_DIRECTORY=$CGROOT;;
    '0::/init.scope')
      PID1_DIRECTORY=$CGROOT/init.scope
      INIT_LOAD=$(linux_systemd_property LoadState init.scope) || return 1
      INIT_STATE=$(linux_systemd_property ActiveState init.scope) || return 1
      INIT_GROUP=$(linux_systemd_property ControlGroup init.scope) || return 1
      [ "$INIT_LOAD:$INIT_STATE" = loaded:active ] || return 1
      # Unit empty means no realized unit path record, NOT the manager root.
      # Kernel membership below independently proves the exact PID1 directory.
      case "$INIT_GROUP" in ''|/init.scope) ;; *) return 1;; esac;;
    *) return 1;;
  esac
  [ -d "$PID1_DIRECTORY" ] && [ ! -L "$PID1_DIRECTORY" ] || return 1
  [ ! -L "$PID1_DIRECTORY/cgroup.procs" ] || return 1
  for PID1_SAMPLE in 1 2; do
    PID1_MEMBERS=$(cat "$PID1_DIRECTORY/cgroup.procs") || return 1
    PID1_FOUND=0
    while IFS= read -r MEMBER; do
      case "$MEMBER" in ''|*[!0-9]*|0|0*) return 1;; esac
      [ "$MEMBER" != 1 ] || PID1_FOUND=1
    done <<POLARIS_PID1_MEMBERS
$PID1_MEMBERS
POLARIS_PID1_MEMBERS
    [ "$PID1_FOUND" -eq 1 ] || return 1
  done
  PID1_AFTER=$(cat /proc/1/cgroup) || return 1
  [ "$PID1_AFTER" = "$PID1_GROUP" ]
}
linux_validate_service() {
  linux_cgroup_view || return 1
  INSTALL_STAGE=unit-config
  LOAD=$(linux_service_property LoadState) || return 1
  case "$LOAD" in
    not-found) return 0;;
    loaded) ;;
    *) return 1;;
  esac
  FRAGMENT=$(linux_service_property FragmentPath) || return 1
  [ "$FRAGMENT" = "$DESCRIPTOR" ] || return 1
  [ -f "$DESCRIPTOR" ] && [ ! -L "$DESCRIPTOR" ] || return 1
  OWNER=$(stat -c %u "$DESCRIPTOR") || return 1
  MODE=$(stat -c %a "$DESCRIPTOR") || return 1
  [ "$OWNER" = 0 ] || return 1
  case "$MODE" in ''|*[!0-7]*) return 1;; esac
  [ "$((0$MODE & 0022))" -eq 0 ] || return 1
  DROPINS=$(linux_service_property DropInPaths) || return 1
  [ -z "$DROPINS" ] || return 1
  UNIT_USER=$(linux_service_property User) || return 1
  case "$UNIT_USER" in ''|root|0) ;; *) return 1;; esac
  KILL_MODE=$(linux_service_property KillMode) || return 1
  [ "$KILL_MODE" = control-group ] || return 1
  SEND_KILL=$(linux_service_property SendSIGKILL) || return 1
  [ "$SEND_KILL" = yes ] || return 1
  FINAL_KILL=$(linux_service_property FinalKillSignal) || return 1
  [ "$FINAL_KILL" = 9 ] || return 1
  DELEGATE=$(linux_service_property Delegate) || return 1
  [ "$DELEGATE" = no ] || return 1
  SLICE=$(linux_service_property Slice) || return 1
  [ "$SLICE" = system.slice ] || return 1
  CONTROL_GROUP=$(linux_service_property ControlGroup) || return 1
  case "$CONTROL_GROUP" in ''|/system.slice/polaris-helper.service) ;; *) return 1;; esac
}
linux_service_closed() {
  # Recheck the effective configuration and hierarchy after the stop job. The
  # pre-stop snapshot cannot authorize removal if unit properties have changed.
  linux_validate_service || return 1
  linux_cohort_closed
}
linux_cohort_closed() {
  # Shared by the strict regular-unit validator and the finite legacy ownmask
  # dispatcher. A caller must validate its actual unit/mask before entering.
  # Never weaken the namespace/view proof when the canonical name is a mask.
  linux_cgroup_view || return 1
  INSTALL_STAGE=service-closure
  STATE=$(linux_service_property ActiveState) || return 1
  PID=$(linux_service_property MainPID) || return 1
  case "$STATE:$PID" in inactive:0|failed:0) ;; *) return 1;; esac
  CONTROL_GROUP=$(linux_service_property ControlGroup) || return 1
  case "$CONTROL_GROUP" in ''|/system.slice/polaris-helper.service) ;; *) return 1;; esac
  # cgroup.events populated covers descendants as well as the unit's own tasks.
  # Do not substitute MainPID=0 or cgroup.procs for this hierarchical proof.
  CGPARENT="$CGROOT/system.slice"
  [ -d "$CGPARENT" ] && [ ! -L "$CGPARENT" ] && [ -r "$CGPARENT" ] && [ -x "$CGPARENT" ] || return 1
  CGROUP="$CGPARENT/polaris-helper.service"
  [ ! -L "$CGROUP" ] || return 1
  # A failed path lookup is not absence. Require a complete, successful listing
  # of this one parent, without descending or scanning unrelated processes.
  CHILDREN=$(find "$CGPARENT" -mindepth 1 -maxdepth 1 -printf '%f\n') || return 1
  FOUND=0
  while IFS= read -r CHILD; do
    if [ "$CHILD" = polaris-helper.service ]; then FOUND=$((FOUND + 1)); fi
  done <<POLARIS_CGROUP_CHILDREN
$CHILDREN
POLARIS_CGROUP_CHILDREN
  [ "$FOUND" -le 1 ] || return 1
  if [ "$FOUND" -eq 0 ]; then return 0; fi
  [ -d "$CGROUP" ] || return 1
  [ -f "$CGROUP/cgroup.events" ] && [ ! -L "$CGROUP/cgroup.events" ] || return 1
  EVENTS=$(cat "$CGROUP/cgroup.events") || return 1
  POPULATED=unknown
  while read -r KEY VALUE EXTRA; do
    if [ "$KEY" = populated ]; then
      [ "$POPULATED" = unknown ] && [ "$VALUE" = 0 ] && [ -z "$EXTRA" ] || return 1
      POPULATED=empty
    fi
  done <<POLARIS_CGROUP_EVENTS
$EVENTS
POLARIS_CGROUP_EVENTS
  [ "$POPULATED" = empty ]
}
stop_service() {
  linux_validate_service || return 1
  INSTALL_STAGE=service-stop
  if [ "$LOAD" = loaded ]; then systemctl --system stop "$SERVICE" || return 1; fi
  linux_service_closed
}
restore_service() {
  linux_cgroup_view || return 1
  systemctl daemon-reload || return 1
  linux_validate_service || return 1
  if [ "$WAS_ENABLED" -eq 1 ]; then systemctl enable "$SERVICE" || return 1;
  elif [ "$HAD_DESCRIPTOR" -eq 1 ]; then systemctl disable "$SERVICE" || return 1; fi
  if [ "$WAS_ACTIVE" -eq 1 ]; then systemctl restart "$SERVICE" || return 1; fi
}
INSTALL_STAGE=service-snapshot
INITIAL_STATE=$(linux_service_property ActiveState) || { echo HELPER_SERVICE_STATE_UNKNOWN >&2; exit 1; }
case "$INITIAL_STATE" in
  active) WAS_ACTIVE=1;;
  inactive|failed) ;;
  *) echo HELPER_SERVICE_STATE_UNKNOWN >&2; exit 1;;
esac
if systemctl is-enabled --quiet "$SERVICE"; then WAS_ENABLED=1; fi
cat > "$TXDIR/descriptor" <<'POLARIS_DESCRIPTOR_EOF'
[Unit]
Description=Polaris privileged network helper
Documentation=https://github.com/polaris-arch/Polaris
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/lib/polaris/helper --socket=/run/polaris/helper.sock --authfile=/var/lib/polaris/authorized-uids --coredir=/usr/local/lib/polaris/core
RuntimeDirectory=polaris
RuntimeDirectoryMode=0755
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
POLARIS_DESCRIPTOR_EOF
snapshot_registration
if [ "$KEEP_HELPER" -eq 1 ] && [ "$WAS_ACTIVE" -eq 1 ] && admin retain-probe --support "$SUPPORT" --id 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'; then
trap rollback EXIT
register_user
merge_authorization
set_phase completed
trap cleanup EXIT
echo polaris-helper-core-only-ok
exit 0
fi
trap rollback EXIT
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
merge_authorization
INSTALL_STAGE=service-start
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
ADMITTED=0
set_phase completed
trap cleanup EXIT
echo polaris-helper-install-ok
