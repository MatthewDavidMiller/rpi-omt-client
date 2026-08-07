ARG ALPINE_DIGEST=sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40
ARG RUST_DIGEST=sha256:c4a364ddbf684fe038e6fa6a4f25b30c8dc85247423e0e660676ece0d17be4a2
ARG TARGETARCH

# Build with the exact stable compiler required by rust-toolchain.toml. The
# official image digest is a multi-architecture manifest, so each output is
# compiled against its native musl target without a cross-linker ambiguity.
# hadolint ignore=DL3029,DL3006
FROM --platform=$TARGETPLATFORM rust:1.97.1-alpine3.23@${RUST_DIGEST} AS receiver-builder

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG RPI_OMT_CLIENT_VERSION=unknown

RUN apk add --no-cache alsa-lib-dev musl-dev pkgconf

WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo/ .cargo/
COPY crates/ crates/
ENV RPI_OMT_CLIENT_VERSION=${RPI_OMT_CLIENT_VERSION}
RUN cargo build --locked --release -p omt-receiver \
    && install -Dm755 target/release/omt-receiver /out/receiver/omt-receiver

# Keep the testable/exportable ARM64 target limited to shipped artifacts. The
# Compiler, source, and Cargo intermediates never enter this image.
FROM scratch AS receiver-artifacts
COPY --from=receiver-builder /out/receiver/omt-receiver /omt-receiver

FROM alpine:3.23.5@${ALPINE_DIGEST} AS python-builder

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

RUN apk add --no-cache build-base python3 python3-dev py3-pip py3-setuptools py3-wheel \
    && python3 -m venv /opt/venv
COPY requirements/runtime.txt /tmp/requirements.txt
RUN /opt/venv/bin/python -m pip install --no-cache-dir --require-hashes \
        -r /tmp/requirements.txt

# Build the application wheel with Alpine's own setuptools, so no build backend
# is fetched at image-build time, then install it into the runtime venv. The
# `packages` list in pyproject.toml names only the shipped package, so the
# dev-only preview fakes cannot reach the image.
WORKDIR /src
COPY pyproject.toml LICENSE ./
COPY src/omt_client/ src/omt_client/
RUN python3 -m pip wheel --no-deps --no-build-isolation -w /tmp/wheel . \
    && /opt/venv/bin/python -m pip install --no-cache-dir --no-deps /tmp/wheel/*.whl \
    && /opt/venv/bin/python -m pip uninstall -y pip setuptools wheel

FROM alpine:3.23.5@${ALPINE_DIGEST} AS runtime

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG WEB_PORT=5000
ARG RPI_OMT_CLIENT_VERSION=unknown

RUN apk add --no-cache \
        alsa-lib \
        bash \
        coreutils \
        openssl \
        python3 \
        util-linux \
        zlib \
    && addgroup -S omt \
    && adduser -S -D -H -G omt -h /etc/omt -s /sbin/nologin omt \
    && mkdir -p /app/legal /etc/omt /run/omt \
    && chown omt:omt /etc/omt /run/omt \
    && chmod 700 /run/omt \
    && rm -f \
        /usr/lib/python3.12/lib-dynload/_decimal*.so \
        /usr/lib/libmpdec.so* \
        /usr/lib/libstdc++.so* \
    && python3 -c 'import decimal; assert str(decimal.Decimal(1) / 7).startswith("0.142857")'

COPY --from=python-builder /opt/venv /opt/venv
COPY --from=receiver-artifacts /omt-receiver /usr/local/bin/omt-receiver
COPY deploy/container/ /usr/local/bin/
COPY LICENSE THIRD_PARTY_NOTICES.txt /app/legal/
COPY Cargo.lock /tmp/Cargo.lock
# The generator imports cargo_lock to resolve the receiver's dependency
# closure, so both modules have to reach the image.
COPY scripts/generate-runtime-sbom.py scripts/cargo_lock.py /tmp/

RUN chmod 0755 \
        /usr/local/bin/omt-receiver \
        /usr/local/bin/control-omt.sh \
        /usr/local/bin/start-omt.sh \
        /usr/local/bin/entrypoint.sh \
    && { \
        find /opt/venv/lib -path '*dist-info/licenses/*' -type f \
                ! -path '*/rpi_omt_client-*' -print \
            | sort \
            | while IFS= read -r license_file; do \
                relative="${license_file#*/site-packages/}"; \
                printf '\n\nPYTHON PACKAGE LICENSE FILE: %s\n' "${relative}"; \
                printf '%s\n\n' '----------------------------------------'; \
                cat "${license_file}"; \
            done; \
    } >> /app/legal/THIRD_PARTY_NOTICES.txt \
    && /opt/venv/bin/python /tmp/generate-runtime-sbom.py \
        --output /app/legal/runtime-sbom.cdx.json \
        --cargo-lock /tmp/Cargo.lock \
        --version "${RPI_OMT_CLIENT_VERSION}" \
    && rm /tmp/generate-runtime-sbom.py /tmp/cargo_lock.py /tmp/Cargo.lock \
    && printf '%s\n' "${RPI_OMT_CLIENT_VERSION}" > /app/RPI_OMT_CLIENT_VERSION \
    && { \
        find /app -type f ! -name runtime-sha256.manifest -print0; \
        find /usr/local/bin -maxdepth 1 -type f -print0; \
        find /opt/venv/lib -type d \
                \( -name omt_client -o -name 'rpi_omt_client-*.dist-info' \) \
                -exec find {} -type f -print0 \; ; \
    } | sort -z | xargs -0 sha256sum > /app/runtime-sha256.manifest

ENV HOME=/etc/omt
ENV OMT_CONFIG_DIR=/etc/omt
ENV OMT_STORAGE_PATH=/etc/omt/omt
# Per-boot state, deliberately off the SD-card-backed config volume. compose.yml
# mounts a tmpfs at /run/omt; the entrypoint owns this 0700 directory inside it.
# /run/omt is pre-created and owned above, because /run itself is root-owned and
# this image runs as omt: without it a run that does not mount the tmpfs could
# not create the directory at all.
ENV OMT_RUNTIME_DIR=/run/omt/state
ENV FLASK_APP=omt_client.wsgi:app
ENV PIP_DISABLE_PIP_VERSION_CHECK=1
ENV PYTHONUNBUFFERED=1
ENV WEB_PORT=${WEB_PORT}
ENV PATH="/opt/venv/bin:${PATH}"

LABEL org.opencontainers.image.title="Raspberry Pi OMT Client"
LABEL org.opencontainers.image.version="${RPI_OMT_CLIENT_VERSION}"
LABEL org.opencontainers.image.licenses="MIT"

USER omt
EXPOSE ${WEB_PORT}
VOLUME ["/etc/omt"]

# Surface a wedged gunicorn in `docker ps` and host diagnostics. The listener is
# HTTPS with a self-signed certificate generated on first boot, so the probe
# does not verify it; GET /login is unauthenticated and unthrottled, so it
# cannot consume an operator's rate-limit budget. The start period must outlast
# first-boot key and certificate generation in entrypoint.sh.
HEALTHCHECK --interval=30s --timeout=5s --start-period=45s --retries=3 \
    CMD ["/opt/venv/bin/python", "-c", "import os, ssl, urllib.request; urllib.request.urlopen(f\"https://127.0.0.1:{os.environ['WEB_PORT']}/login\", timeout=4, context=ssl._create_unverified_context())"]
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
