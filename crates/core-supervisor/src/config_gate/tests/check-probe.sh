#!/bin/sh
case "$*" in
*hang*) witness=$(cat "$4"); sleep 0.4; printf done > "$witness"; exit 0;;
*accept*) exit 0;;
*reject*) echo 'FATAL[0000] decode config at cfg.json: outbounds[3]: unknown outbound type: zzz' >&2;;
*stdout*) echo 'FATAL[0000] decode config at cfg.json: outbounds[3]: unknown outbound type: zzz';;
*garbage*) echo 'FATAL[0000] decode config at cfg.json: duplicate outbound/endpoint tag: dup' >&2;;
*argv*) printf '%s\n' "$*" >&2;;
*silent*) :;;
*) exit 0;;
esac
exit 1
