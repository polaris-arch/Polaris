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
: > "$BOOT/original-proxy"
CLASS=$(admin linux-legacy-classify --operation uninstall)
case "$CLASS" in
  legacy)
    admin linux-legacy-uninstall --config '/' --original-file "$BOOT/original-proxy"
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
SERVICE='polaris-helper.service'
GROUP=root
MAINTENANCE_MAC=0
RECORD="$SUPPORT/helper-installed.json"
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
secure_dir '/usr/local/lib/polaris'
secure_dir '/etc/systemd/system'
secure_dir "$SUPPORT"
[ ! -e "$SUPPORT/helper-install.lock" ] && [ ! -L "$SUPPORT/helper-install.lock" ] || { echo LEGACY_HELPER_INSTALL_RECOVERY_REQUIRED >&2; exit 75; }
LOCK="$SUPPORT/helper-install.lease"
if [ ! -e "$LOCK" ]; then (umask 077; set -C; : > "$LOCK") || [ -f "$LOCK" ] || exit 75; fi
[ -f "$LOCK" ] && [ ! -L "$LOCK" ] || { echo HELPER_INSTALL_LOCK_INVALID >&2; exit 1; }
exec 9>>"$LOCK"
set_phase() {
  printf '%s\n' "$1" > "$TXDIR/transaction-phase.next" &&
    admin sync-install-path --path "$TXDIR/transaction-phase.next" &&
    mv -f "$TXDIR/transaction-phase.next" "$TXDIR/transaction-phase" &&
    admin sync-install-path --path "$TXDIR"
}

TXDIR=''
TXDIR_ID=''
FINAL_STATE=unchanged
FINISHED=0
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
  exec 9>&- || cleanup_failure
  BOOTSTRAP_CLOSED=1
  close_bootstrap || { rc=1; BOOTSTRAP_CLOSED=0; }
  if [ "$FINISHED" -eq 1 ] && [ "$rc" -eq 0 ]; then
    echo "polaris-helper-uninstall-state=$FINAL_STATE"
    if [ -n "$TXDIR" ]; then echo "polaris-helper-uninstall-journal=$TXDIR"; fi
    echo polaris-helper-uninstall-ok
  fi
  if [ "${POLARIS_DIAGNOSTICS-0}" = 1 ]; then
    RECOVERY=recovery-unconfirmed
    if [ "$FINISHED" -eq 1 ] && [ "$BOOTSTRAP_CLOSED" -eq 1 ]; then RECOVERY=cleanup-returned; fi
    diagnostic_failure "$rc" "$FAILED_STAGE" "$RECOVERY" || :
  fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
INSTALL_STAGE=lease
admin lease-acquire --lock "$LOCK" --fd 9 || { echo HELPER_INSTALL_TRANSACTION_BUSY >&2; exit 75; }
# Old receipts are evidence only. This operation neither resumes nor deletes
# an interrupted install/uninstall, and never treats a prior stop as authority.
for OLD in "$SUPPORT"/helper-upgrade.*; do
  [ -e "$OLD" ] || [ -L "$OLD" ] || continue
  echo "HELPER_INSTALL_RECOVERY_REQUIRED $OLD" >&2
  exit 75
done
INSTALL_STAGE=journal
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
INSTALLDIR='/usr/local/lib/polaris'
RUNTIMEDIR='/run/polaris'

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

set_phase uninstall-stop-started
FINAL_STATE=stop-unconfirmed
linux_validate_service || { echo HELPER_UNINSTALL_SERVICE_STATE_UNKNOWN >&2; exit 1; }
INSTALL_STAGE=service-stop
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
    "$LOCK"|"$TXDIR"|"$SUPPORT"/helper-upgrade.*|"$SUPPORT"/helper-admission.*|"$SUPPORT"/helper-retired.*|"$SUPPORT/helper-legacy-tombstone") continue;;
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
