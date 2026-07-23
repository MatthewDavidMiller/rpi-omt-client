#!/bin/bash
# Detect the release version to bake into the Docker image.

set -euo pipefail

PROJECT_ROOT="${1:-$(pwd)}"

if [[ -n "${RPI_OMT_CLIENT_VERSION:-}" ]]; then
    printf '%s\n' "${RPI_OMT_CLIENT_VERSION}"
    exit 0
fi

if git -C "${PROJECT_ROOT}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    if version="$(git -C "${PROJECT_ROOT}" describe --tags --exact-match 2>/dev/null)" \
        && [[ -n "${version}" ]]; then
        printf '%s\n' "${version}"
        exit 0
    fi
fi

project_dir="$(basename "${PROJECT_ROOT}")"
if [[ "${project_dir}" =~ (^|[-_])(v?[0-9]+(\.[0-9]+){1,2}([._-][0-9A-Za-z][0-9A-Za-z._-]*)?)$ ]]; then
    printf '%s\n' "${BASH_REMATCH[2]}"
    exit 0
fi

printf 'unknown\n'
