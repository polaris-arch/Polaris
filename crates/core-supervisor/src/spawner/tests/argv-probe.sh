#!/bin/sh
case "$3" in
--sleep) exec sleep 30;;
--pwd) pwd;;
--flood-stderr:*) n=${3#*:}; head -c "$n" /dev/zero >&2;;
*) printf '%s' "$1"; shift; for v in "$@"; do printf ' %s' "$v"; done; printf '\n';;
esac
