//! Administrator scripts share byte sealing and upgrade admission. Platform
//! service operations remain explicit so rollback restores the original service.
use super::*;

fn transaction(paths: &InstallPaths, params: &InstallParams, token: &str, mac: bool) -> String {
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
    let header = format!(
        r#"#!/bin/sh
set -eu
umask 077
DEST={dest}
DESCRIPTOR={descriptor}
SUPPORT={support}
COREDIR={core}
SERVICE={service}
GROUP={group}
secure_dir() {{
  mkdir -p "$1"
  if [ -L "$1" ] || [ ! -d "$1" ]; then echo "invalid installation directory: $1" >&2; exit 1; fi
  chown -h root:"$GROUP" "$1"
  chmod 0755 "$1"
}}
secure_dir {parent}
secure_dir "$SUPPORT"
secure_dir "$COREDIR"
LOCK="$SUPPORT/helper-install.lock"
if ! mkdir -m 0700 "$LOCK"; then echo HELPER_INSTALL_TRANSACTION_BUSY >&2; exit 75; fi
TXDIR=''
ADMITTED=0
MUTATED=0
HAD_HELPER=0
HAD_DESCRIPTOR=0
HAD_RECORD=0
WAS_ACTIVE=0
WAS_ENABLED=0
RECORD="$SUPPORT/helper-installed.json"
cleanup() {{
  rc=$?
  trap - EXIT HUP INT TERM
  if [ "$ADMITTED" -eq 1 ]; then "$TXDIR/helper" cancel-upgrade --support "$SUPPORT" >/dev/null 2>&1 || true; fi
  if [ -n "$TXDIR" ]; then rm -rf "$TXDIR"; fi
  rmdir "$LOCK"
  exit "$rc"
}}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
TXDIR=$(mktemp -d "$SUPPORT/helper-upgrade.XXXXXX")
chmod 0700 "$TXDIR"
cp {source} "$TXDIR/helper"
chown root:"$GROUP" "$TXDIR/helper"; chmod 0755 "$TXDIR/helper"
cp {manifest} "$TXDIR/core-manifest.json"
chown root:"$GROUP" "$TXDIR/core-manifest.json"; chmod 0600 "$TXDIR/core-manifest.json"
hash_file() {{ {hash_command}; }}
[ "$(hash_file "$TXDIR/helper")" = {helper_hash} ] || {{ echo HELPER_BUNDLE_MISMATCH >&2; exit 1; }}
[ "$(hash_file "$TXDIR/core-manifest.json")" = {manifest_hash} ] || {{ echo CORE_BUNDLE_MISMATCH >&2; exit 1; }}
printf '%s' {token} > "$TXDIR/user-token"
chmod 0600 "$TXDIR/user-token"
"$TXDIR/helper" core-install --root "$COREDIR" --source {payload} --manifest "$TXDIR/core-manifest.json" --source-owner {identity}
"$TXDIR/helper" register-user --support "$SUPPORT" --identity {identity} --confdir {config} --token-file "$TXDIR/user-token"
POLICY=$("$TXDIR/helper" helper-policy --binary "$DEST" --metadata "$RECORD")
case "$POLICY" in
  'helper-policy retain') echo polaris-helper-core-only-ok; exit 0;;
  'helper-policy replace'|'helper-policy replace-legacy') ;;
  *) echo HELPER_INSTALLED_RECEIPT_INVALID >&2; exit 1;;
esac
for OBJECT in "$DEST" "$DESCRIPTOR" "$RECORD"; do
  if [ -L "$OBJECT" ] || {{ [ -e "$OBJECT" ] && [ ! -f "$OBJECT" ]; }}; then echo "invalid service object: $OBJECT" >&2; exit 1; fi
done
if [ -f "$DEST" ]; then cp "$DEST" "$TXDIR/helper.rollback"; HAD_HELPER=1; fi
if [ -f "$DESCRIPTOR" ]; then cp "$DESCRIPTOR" "$TXDIR/descriptor.rollback"; HAD_DESCRIPTOR=1; fi
if [ -f "$RECORD" ]; then cp "$RECORD" "$TXDIR/record.rollback"; HAD_RECORD=1; fi
rollback() {{
  rc=$?
  trap - EXIT HUP INT TERM
  OK=1
  if [ "$MUTATED" -eq 1 ]; then
    stop_service || OK=0
    if [ "$HAD_HELPER" -eq 1 ]; then
      install -o root -g "$GROUP" -m 0755 "$TXDIR/helper.rollback" "$DEST.new.$$" && mv -f "$DEST.new.$$" "$DEST" || OK=0
    else rm -f "$DEST" || OK=0; fi
    if [ "$HAD_DESCRIPTOR" -eq 1 ]; then
      install -o root -g "$GROUP" -m 0644 "$TXDIR/descriptor.rollback" "$DESCRIPTOR.new.$$" && mv -f "$DESCRIPTOR.new.$$" "$DESCRIPTOR" || OK=0
    else rm -f "$DESCRIPTOR" || OK=0; fi
    if [ "$HAD_RECORD" -eq 1 ]; then
      install -o root -g "$GROUP" -m 0644 "$TXDIR/record.rollback" "$RECORD.new.$$" && mv -f "$RECORD.new.$$" "$RECORD" || OK=0
    else rm -f "$RECORD" || OK=0; fi
    restore_service || OK=0
  elif [ "$ADMITTED" -eq 1 ]; then
    "$TXDIR/helper" cancel-upgrade --support "$SUPPORT" >/dev/null 2>&1 || OK=0
  fi
  rm -f "$DEST.new.$$" "$DESCRIPTOR.new.$$" "$RECORD.new.$$"
  if [ "$OK" -eq 1 ]; then rm -rf "$TXDIR"; rmdir "$LOCK"; else echo "helper rollback incomplete; recovery transaction retained: $TXDIR" >&2; fi
  exit "$rc"
}}
"#,
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
        token = shell_quote(token),
        payload = shell_quote(
            &params
                .bundled_core
                .parent()
                .expect("staged core parent")
                .to_string_lossy()
        ),
        identity = shell_quote(&params.user_identity.key()),
        config = shell_quote(&params.conf_dir.to_string_lossy()),
    );
    let authorization = if mac {
        String::new()
    } else {
        format!(
            r#"AUTHFILE={authfile}
if [ -L "$AUTHFILE" ] || {{ [ -e "$AUTHFILE" ] && [ ! -f "$AUTHFILE" ]; }}; then echo invalid-authfile >&2; exit 1; fi
if [ ! -e "$AUTHFILE" ]; then (umask 077; touch "$AUTHFILE"); fi
chown -h root:root "$AUTHFILE"; chmod 0600 "$AUTHFILE"
grep -qxF {uid} "$AUTHFILE" || printf '%s\n' {uid} >> "$AUTHFILE"
"#,
            authfile = shell_quote(LINUX_AUTH_FILE),
            uid = shell_quote(&params.uid.to_string())
        )
    };
    let header = header.replace("POLICY=$(", &format!("{authorization}POLICY=$("));
    let platform = if mac {
        let plist = render_mac_plist(&program, &core, support);
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
"#
        )
    } else {
        let unit = build_linux_unit(paths);
        format!(
            r#"stop_service() {{
  systemctl stop "$SERVICE" || return 1
  ! systemctl is-active --quiet "$SERVICE"
}}
restore_service() {{
  systemctl daemon-reload || return 1
  if [ "$WAS_ENABLED" -eq 1 ]; then systemctl enable "$SERVICE" || return 1;
  elif [ "$HAD_DESCRIPTOR" -eq 1 ]; then systemctl disable "$SERVICE" || return 1; fi
  if [ "$WAS_ACTIVE" -eq 1 ]; then systemctl restart "$SERVICE" || return 1; fi
}}
if systemctl is-active --quiet "$SERVICE"; then WAS_ACTIVE=1; fi
if systemctl is-enabled --quiet "$SERVICE"; then WAS_ENABLED=1; fi
cat > "$TXDIR/descriptor" <<'POLARIS_DESCRIPTOR_EOF'
{unit}POLARIS_DESCRIPTOR_EOF
"#
        )
    };
    let admission = r#"trap rollback EXIT
if [ "$WAS_ACTIVE" -eq 1 ]; then
  if ! "$TXDIR/helper" prepare-upgrade --support "$SUPPORT"; then
    echo HELPER_UPGRADE_DEFERRED_ACTIVE_OR_LEGACY >&2
    exit 75
  fi
  ADMITTED=1
fi
MUTATED=1
if [ "$WAS_ACTIVE" -eq 1 ]; then stop_service; fi
install -o root -g "$GROUP" -m 0755 "$TXDIR/helper" "$DEST.new.$$"
mv -f "$DEST.new.$$" "$DEST"
install -o root -g "$GROUP" -m 0644 "$TXDIR/descriptor" "$DESCRIPTOR.new.$$"
mv -f "$DESCRIPTOR.new.$$" "$DESCRIPTOR"
"$TXDIR/helper" helper-record --binary "$DEST" --metadata "$RECORD"
"#;
    let takeover = if mac {
        r#"launchctl enable "system/$SERVICE"
launchctl bootstrap system "$DESCRIPTOR"
launchctl print "system/$SERVICE" >/dev/null
"#
    } else {
        r#"systemctl daemon-reload
systemctl enable "$SERVICE"
systemctl restart "$SERVICE"
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
    format!("{header}{platform}{admission}{takeover}ADMITTED=0\ntrap cleanup EXIT\necho polaris-helper-install-ok\n")
}

pub(super) fn build_mac_install_script(
    paths: &InstallPaths,
    params: &InstallParams,
    token: &str,
) -> String {
    transaction(paths, params, token, true)
}
pub(super) fn build_linux_install_script(
    paths: &InstallPaths,
    params: &InstallParams,
    token: &str,
) -> String {
    transaction(paths, params, token, false)
}
