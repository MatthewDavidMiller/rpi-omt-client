#!/bin/bash
# Run a gate command inside the toolbox image.
#
# Usage: ./scripts/toolbox.sh <command> [args...]
#        ./scripts/toolbox.sh --build          Build (or rebuild) the image only
#        ./scripts/toolbox.sh --shell          Interactive shell in the toolbox
#
# Docker or Podman is the only thing this needs from the host. Every compiler,
# linter, scanner, and test runner the gates call lives in tools/toolbox.
#
# The repository is bind-mounted at the same absolute path it occupies on the
# host, not at a fixed /work. Gates that start their own containers hand paths
# to the host engine through the mounted socket, and the host resolves those
# paths in its own filesystem, so any other mount point would make a nested
# `-v "${PROJECT_ROOT}/x:/y"` silently mount the wrong directory.

set -euo pipefail

# Already inside the toolbox: run the command rather than nesting a container.
# The gates chain into each other and each entry point routes through here, so
# this is the ordinary case during a gate run, not an edge case.
if [[ -n "${OMT_IN_TOOLBOX:-}" && "${1:-}" != "--build" ]]; then
    [[ $# -gt 0 ]] || { echo "Usage: $0 <command> [args...]" >&2; exit 2; }
    exec "$@"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=scripts/docker-test-env.sh
source "${SCRIPT_DIR}/docker-test-env.sh"

DOCKERFILE="${PROJECT_ROOT}/tools/toolbox/Dockerfile"
CARGO_VOLUME="${OMT_TOOLBOX_CARGO_VOLUME:-omt-toolbox-cargo}"
TOOLBOX_REPO=omt-toolbox

# The tag is the digest of everything that determines what lands in the image.
# A changed pin, linter version, or Python requirement therefore names a
# different image, and the rebuild happens on the next gate run rather than
# whenever somebody remembers to force one.
toolbox_tag() {
    local digest
    digest="$(
        cat \
            "${DOCKERFILE}" \
            "${PROJECT_ROOT}/tests/requirements-dev.txt" \
            "${PROJECT_ROOT}/scripts/install-hadolint.sh" \
            "${PROJECT_ROOT}/scripts/install-trivy.sh" |
            sha256sum | cut -c1-16
    )"
    printf '%s:%s\n' "${TOOLBOX_REPO}" "${digest}"
}

# Drop the builds this one replaces.
#
# Content-hash tags mean a bumped pin names a new image and silently strands
# the old one, and the toolbox is roughly 4 GB apiece. Without this a year of
# routine version bumps leaves tens of gigabytes of images nothing will ever
# run again.
#
# Failures are ignored on purpose: `rmi` refuses an image a container is still
# using, which is exactly what a gate running concurrently in the previous
# toolbox looks like. That one survives and is collected on the next build.
prune_superseded_toolboxes() {
    local keep="$1" image removed=0
    while read -r image; do
        [[ -n "${image}" && "${image}" != "${keep}" ]] || continue
        if "${CONTAINER_ENGINE}" rmi "${image}" >/dev/null 2>&1; then
            removed=$((removed + 1))
        fi
    done < <(
        "${CONTAINER_ENGINE}" images --format '{{.Repository}}:{{.Tag}}' 2>/dev/null |
            grep "^${TOOLBOX_REPO}:" || true
    )
    ((removed > 0)) && echo "Removed ${removed} superseded toolbox image(s)." >&2
    return 0
}

build_toolbox() {
    local tag="$1"
    echo "=== Building toolbox image ${tag} ===" >&2
    # The build context is the repository root: the image copies the pinned
    # installer scripts and the Python requirements out of the tree itself.
    container_engine_build \
        -f "${DOCKERFILE}" \
        -t "${tag}" \
        "${PROJECT_ROOT}"
    # Only after the build succeeded: a failed one leaves the previous image as
    # the only working toolbox on the machine.
    prune_superseded_toolboxes "${tag}"
}

main() {
    local tag mode="run"
    case "${1:-}" in
        --build) mode="build"; shift ;;
        --shell) mode="shell"; shift ;;
        -h|--help)
            sed -n '2,16p' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        "")
            echo "Usage: $0 <command> [args...]" >&2
            exit 2
            ;;
    esac

    ensure_test_container_engine || exit 1
    tag="$(toolbox_tag)"

    if [[ "${mode}" == "build" ]]; then
        build_toolbox "${tag}"
        exit 0
    fi

    if ! "${CONTAINER_ENGINE}" image inspect "${tag}" >/dev/null 2>&1; then
        build_toolbox "${tag}"
    fi

    local -a engine_args=(
        run --rm
        -v "$(container_engine_volume "${PROJECT_ROOT}" "${PROJECT_ROOT}")"
        -v "${CARGO_VOLUME}:/cargo"
        # The host's /tmp, for the same reason the repository is mounted at its
        # own path. Gates build fixtures under `mktemp -d` and bind-mount them
        # into containers they start; those paths are resolved by the host
        # daemon, so a private /tmp in here means it creates an empty
        # root-owned directory instead and the fixture is silently missing.
        # Not relabelled under Podman: /tmp is shared with the rest of the
        # system and relabelling it would affect far more than this container.
        -v /tmp:/tmp
        # Containers a gate starts publish their ports on the host, which is
        # not this container's loopback, so the smoke test talks to the
        # appliance on the engine network instead of through its published
        # port.
        #
        # Deliberately not --network host, which would make the published port
        # reachable but also makes /proc/sys/net the host's, where the
        # hardening sysctls the diagnostics gate reports on are root-owned and
        # mode 0600 -- unreadable to the mapped user, silently dropped by
        # sysctl, and absent from the report that gate then inspects. Nor a
        # host-gateway route: that depends on the workstation's firewall
        # allowing bridge-to-host traffic, which this one does not.
        -e OMT_SMOKE_VIA_ENGINE_NETWORK=1
        -w "${PROJECT_ROOT}"
        -e HOME=/cargo
    )
    # CONTAINER_ENGINE is deliberately not forwarded. scripts/docker-test-env.sh
    # treats it as an explicit request that overrides detection, and
    # tests/unit/test_container_engine.sh proves that detection by building
    # fake engines on PATH -- a value inherited from out here would decide the
    # answer before the test could.

    # Only when the caller actually set it. Forwarding it unconditionally would
    # define it as empty inside, and scripts/detect-version.sh distinguishes
    # "no override" from "an override I cannot parse".
    if [[ -n "${RPI_OMT_CLIENT_VERSION:-}" ]]; then
        engine_args+=(-e "RPI_OMT_CLIENT_VERSION=${RPI_OMT_CLIENT_VERSION}")
    fi

    # tests/unit/test_firewall_reachability.sh proves the appliance's nftables
    # rules against a real network stack by unsharing a private user, mount,
    # and network namespace -- which grants it CAP_NET_ADMIN over its own
    # throwaway stack and nothing else. The default seccomp profile refuses
    # the nested `unshare`, so the gate would report a missing prerequisite
    # rather than a result.
    #
    # Mounting sysfs inside that namespace additionally needs the engine's
    # masked /proc and /sys paths lifted. Deliberately not --privileged and
    # deliberately no --cap-add: neither was enough on its own, and unmasking
    # plus the seccomp relaxation is, so the container still runs as the
    # invoking user holding no extra capabilities.
    #
    # This is also not the widest door open here -- the Docker socket mounted
    # below already amounts to host control -- and the gates only ever run this
    # repository's own code.
    engine_args+=(--security-opt seccomp=unconfined)
    if [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]; then
        engine_args+=(--security-opt unmask=all)
    else
        engine_args+=(--security-opt systempaths=unconfined)
    fi

    # Keep files the gates write into the tree owned by the invoking user
    # rather than by root. Podman rootless already maps the caller to the
    # container's root, so it needs the inverse instruction.
    if [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]; then
        engine_args+=(--userns=keep-id)
    else
        engine_args+=(--user "$(id -u):$(id -g)")
    fi

    # Image builds and Trivy image scans run against the host engine through
    # its socket; there is no daemon inside the toolbox. Podman's socket lives
    # elsewhere and speaks the Docker API, so it is mounted at the path the
    # toolbox's docker CLI looks for.
    local socket=""
    if [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]; then
        for candidate in \
            "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock" \
            /run/podman/podman.sock; do
            [[ -S "${candidate}" ]] && { socket="${candidate}"; break; }
        done
    elif [[ -S /var/run/docker.sock ]]; then
        socket=/var/run/docker.sock
    fi
    if [[ -n "${socket}" ]]; then
        engine_args+=(-v "$(container_engine_volume "${socket}" /var/run/docker.sock)")
        # A mapped non-root user cannot read the socket without holding the
        # group that owns it. Rootless Podman's socket is already owned by the
        # caller.
        if [[ "${CONTAINER_ENGINE_KIND}" == "docker" ]]; then
            engine_args+=(--group-add "$(stat -c %g "${socket}")")
        fi
    else
        echo "WARNING: no container engine socket found; gates that build images will fail" >&2
    fi

    if [[ "${mode}" == "shell" ]]; then
        engine_args+=(-it "${tag}" bash)
    else
        engine_args+=("${tag}" "$@")
    fi

    exec "${CONTAINER_ENGINE}" "${engine_args[@]}"
}

main "$@"
