#!/bin/sh
# See cargo-linker.sh.
exec "$(dirname "$0")/cargo-linker.sh" x86_64 "$@"
