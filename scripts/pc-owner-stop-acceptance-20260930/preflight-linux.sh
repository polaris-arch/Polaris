#!/bin/sh
# Reads selected installed files and host metadata; does not execute App/helper/core.
set -eu
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
exec python3 "$script_dir/acceptance.py" preflight --platform linux "$@"
