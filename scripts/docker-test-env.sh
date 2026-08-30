#!/bin/bash
# Shared Docker/Podman readiness checks for tests that run live containers.

CONTAINER_ENGINE="${CONTAINER_ENGINE:-}"
CONTAINER_ENGINE_KIND="${CONTAINER_ENGINE_KIND:-}"
CONTAINER_ENGINE_ANNOUNCED="${CONTAINER_ENGINE_ANNOUNCED:-0}"

set_test_container_engine() {
    local engine_path="$1"
    local engine_kind="$2"
    local display_name

    CONTAINER_ENGINE="${engine_path}"
    CONTAINER_ENGINE_KIND="${engine_kind}"
    export CONTAINER_ENGINE CONTAINER_ENGINE_KIND

    if [[ "${CONTAINER_ENGINE_ANNOUNCED}" != "1" ]]; then
        case "${engine_kind}" in
            docker) display_name="Docker" ;;
            podman) display_name="Podman" ;;
            *) display_name="${engine_kind}" ;;
        esac
        echo "Using ${display_name} for live container tests: ${engine_path}" >&2
        CONTAINER_ENGINE_ANNOUNCED=1
        export CONTAINER_ENGINE_ANNOUNCED
    fi
}

# Identify the client a path actually runs, rather than the name it is
# installed under.
#
# `podman-docker` ships a /usr/bin/docker that execs Podman, so the name says
# Docker while every flag chosen from this kind -- `--format docker`, the `:Z`
# relabel, and the user mapping in scripts/toolbox.sh -- has to be the Podman
# spelling. Asking the client to identify itself also stays correct in the
# inverse case, which matters just as much: the toolbox image ships a real
# Docker CLI that drives the workstation's Podman socket, and that one must
# keep the Docker spelling even though the server is Podman.
#
# An unrecognized `--version` falls back to the installed name. The answer is
# then no worse than the one this made before asking.
container_engine_kind() {
    local engine_path="$1"
    local engine_name version
    engine_name="$(basename "${engine_path}")"
    case "${engine_name}" in
        docker|podman) ;;
        *) return 1 ;;
    esac

    version="$("${engine_path}" --version 2>/dev/null || true)"
    case "${version}" in
        [Pp]odman*) printf '%s\n' podman ;;
        [Dd]ocker*) printf '%s\n' docker ;;
        *) printf '%s\n' "${engine_name}" ;;
    esac
}

container_engine_volume() {
    local host_path="$1"
    local container_path="$2"
    local access_mode="${3:-}"
    local options="${access_mode}"

    # Rootless Podman on SELinux hosts needs a private relabel for test-owned
    # bind mounts. Docker retains its existing mount behavior.
    if [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]; then
        if [[ -n "${options}" ]]; then
            options="${options},Z"
        else
            options="Z"
        fi
    fi

    if [[ -n "${options}" ]]; then
        printf '%s:%s:%s\n' "${host_path}" "${container_path}" "${options}"
    else
        printf '%s:%s\n' "${host_path}" "${container_path}"
    fi
}

container_engine_build() {
    if [[ "${CONTAINER_ENGINE_KIND}" == "podman" ]]; then
        # Podman defaults to OCI image metadata, which ignores Dockerfile SHELL
        # instructions. Docker format preserves the Docker build contract.
        "${CONTAINER_ENGINE}" build --format docker "$@"
    else
        "${CONTAINER_ENGINE}" build "$@"
    fi
}

ensure_docker_daemon() {
    local current_uid docker_info_output
    current_uid="${EUID:-}"
    if [[ -z "${current_uid}" ]]; then
        current_uid="$(id -u)"
    fi

    if ! command -v docker >/dev/null 2>&1; then
        echo "FAIL: Docker is not installed or not on PATH." >&2
        return 1
    fi

    if docker info >/dev/null 2>&1; then
        return 0
    fi

    docker_info_output="$(docker info 2>&1 || true)"
    if grep -qi 'permission denied' <<< "${docker_info_output}"; then
        echo "FAIL: Docker daemon is running, but this user cannot access the Docker socket." >&2
        echo "      Check Docker socket permissions, group membership, or sandbox access." >&2
        return 1
    fi

    echo "Docker daemon is not reachable; attempting to start it..." >&2
    if command -v systemctl >/dev/null 2>&1; then
        if [[ "${current_uid}" -eq 0 ]]; then
            systemctl start docker >/dev/null 2>&1 || true
        elif command -v sudo >/dev/null 2>&1; then
            sudo -n systemctl start docker >/dev/null 2>&1 || true
        fi
    fi

    if ! docker info >/dev/null 2>&1 && command -v service >/dev/null 2>&1; then
        if [[ "${current_uid}" -eq 0 ]]; then
            service docker start >/dev/null 2>&1 || true
        elif command -v sudo >/dev/null 2>&1; then
            sudo -n service docker start >/dev/null 2>&1 || true
        fi
    fi

    if docker info >/dev/null 2>&1; then
        echo "Docker daemon is running." >&2
        return 0
    fi

    docker_info_output="$(docker info 2>&1 || true)"
    if grep -qi 'permission denied' <<< "${docker_info_output}"; then
        echo "FAIL: Docker daemon is running, but this user cannot access the Docker socket." >&2
        echo "      Check Docker socket permissions, group membership, or sandbox access." >&2
    else
        echo "FAIL: Docker daemon is not running and could not be started non-interactively." >&2
    fi
    return 1
}

ensure_test_container_engine() {
    local docker_path=""
    local docker_kind=""
    local podman_path=""
    local requested_engine="${CONTAINER_ENGINE}"
    local requested_kind=""
    local requested_path=""
    local podman_info_output=""

    if [[ -n "${requested_engine}" ]]; then
        requested_path="$(command -v "${requested_engine}" 2>/dev/null || true)"
        if [[ -z "${requested_path}" ]]; then
            echo "FAIL: Requested container engine is not installed or not on PATH: ${requested_engine}" >&2
            return 1
        fi
        requested_kind="$(container_engine_kind "${requested_path}" 2>/dev/null || true)"
        if [[ -z "${requested_kind}" ]]; then
            echo "FAIL: CONTAINER_ENGINE must resolve to Docker or Podman: ${requested_engine}" >&2
            return 1
        fi
        if "${requested_path}" info >/dev/null 2>&1; then
            set_test_container_engine "${requested_path}" "${requested_kind}"
            return 0
        fi
        if [[ "${requested_kind}" == "docker" ]]; then
            if ensure_docker_daemon; then
                docker_path="$(command -v docker)"
                set_test_container_engine "${docker_path}" docker
                return 0
            fi
        else
            podman_info_output="$("${requested_path}" info 2>&1 || true)"
            echo "FAIL: Podman is installed but is not usable by this user." >&2
            [[ -n "${podman_info_output}" ]] && echo "      ${podman_info_output}" >&2
        fi
        return 1
    fi

    docker_path="$(command -v docker 2>/dev/null || true)"
    podman_path="$(command -v podman 2>/dev/null || true)"

    # Match the client to the server when the caller knows what the server is.
    #
    # The toolbox installs both clients and a Podman socket answers either, so
    # first-found is not good enough there: a Podman engine driven by the
    # Docker CLI builds OCI images, and an OCI image config has nowhere to
    # store a HEALTHCHECK. The appliance image then ships without the probe
    # deploy/Dockerfile declares, and the only thing that notices is the smoke
    # gate, one build too late. `--format docker` fixes it and only Podman's
    # own client can pass it.
    if [[ "${OMT_ENGINE_SERVER_KIND:-}" == "podman" && -n "${podman_path}" ]] &&
       "${podman_path}" info >/dev/null 2>&1; then
        set_test_container_engine "${podman_path}" podman
        return 0
    fi

    if [[ -n "${docker_path}" ]] && "${docker_path}" info >/dev/null 2>&1; then
        docker_kind="$(container_engine_kind "${docker_path}")"
        # A `docker` that is really podman-docker's shim: run the engine under
        # its own name when it is installed, so the gate announces what it
        # actually drives and no output is prefixed by the shim's notice.
        if [[ "${docker_kind}" == "podman" && -n "${podman_path}" ]]; then
            set_test_container_engine "${podman_path}" podman
        else
            set_test_container_engine "${docker_path}" "${docker_kind}"
        fi
        return 0
    fi

    if [[ -n "${podman_path}" ]] && "${podman_path}" info >/dev/null 2>&1; then
        set_test_container_engine "${podman_path}" podman
        return 0
    fi

    # Preserve the existing Docker behavior when it is the only installed
    # option: attempt to start its daemon without prompting.
    if [[ -n "${docker_path}" ]] && ensure_docker_daemon; then
        set_test_container_engine "${docker_path}" "$(container_engine_kind "${docker_path}")"
        return 0
    fi

    if [[ -n "${podman_path}" ]]; then
        podman_info_output="$("${podman_path}" info 2>&1 || true)"
        echo "FAIL: Podman is installed but is not usable by this user." >&2
        [[ -n "${podman_info_output}" ]] && echo "      ${podman_info_output}" >&2
    elif [[ -z "${docker_path}" ]]; then
        echo "FAIL: Neither Docker nor Podman is installed or on PATH." >&2
    fi
    return 1
}
