//! Linux service closure is separate from launchd's process observation model.
//! Query the effective system unit before stopping it; then require hierarchical
//! cgroup-v2 emptiness. Unknown or overridden configurations retain all files.
pub(super) const FUNCTIONS: &str = r#"
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
  INSTALL_STAGE=pid1-cgroup
  PID1_GROUP=$(cat /proc/1/cgroup) || return 1
  case "$PID1_GROUP" in
    '0::/') ;;
    '0::/init.scope')
      INIT_GROUP=$(linux_systemd_property ControlGroup init.scope) || return 1
      [ "$INIT_GROUP" = /init.scope ] || return 1;;
    *) return 1;;
  esac
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
  [ "$MATCHES" -eq 1 ]
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
"#;

/// Fixed failure-only observation; never authorization or cleanup proof.
pub(super) const DIAGNOSTICS: &str = r#"
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
"#;
