ARG DOTNET_SDK_ALPINE_DIGEST=sha256:d8ee39817ca03a3757288e83c37ed73cc969a286c603b827c7cbe33add1c2d1c
ARG ALPINE_DIGEST=sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40

# NativeAOT and libvmx are compiled natively for the target platform. ARM64
# builds therefore use the repository's checked buildx/QEMU prerequisite.
FROM --platform=$TARGETPLATFORM mcr.microsoft.com/dotnet/sdk:10.0-alpine3.23@${DOTNET_SDK_ALPINE_DIGEST} AS receiver-builder

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG TARGETARCH
ARG RPI_OMT_CLIENT_VERSION=unknown

RUN apk add --no-cache build-base clang zlib-dev

WORKDIR /src
COPY Directory.Build.props global.json ./
COPY receiver/ receiver/
COPY third_party/omt/ third_party/omt/

# hadolint ignore=SC2016
RUN mkdir -p /out/native \
    && if [ "${TARGETARCH}" = "arm64" ]; then \
        clang++ -O3 -std=c++17 -fdeclspec -fPIC -shared \
            third_party/omt/libvmx/src/vmxcodec_arm.cpp \
            third_party/omt/libvmx/src/vmxcodec.cpp \
            -Wl,-rpath,'$ORIGIN' -o /out/native/libvmx.so; \
        runtime_id=linux-musl-arm64; \
    elif [ "${TARGETARCH}" = "amd64" ]; then \
        clang++ -O3 -std=c++17 -fdeclspec -fPIC -mlzcnt -mavx2 -mbmi -shared \
            third_party/omt/libvmx/src/vmxcodec_x86.cpp \
            third_party/omt/libvmx/src/vmxcodec_avx2.cpp \
            third_party/omt/libvmx/src/vmxcodec.cpp \
            -Wl,-rpath,'$ORIGIN' -o /out/native/libvmx.so; \
        runtime_id=linux-musl-x64; \
    else \
        echo "Unsupported target architecture: ${TARGETARCH}" >&2; exit 1; \
    fi \
    && dotnet publish receiver/RpiOmt.Receiver/RpiOmt.Receiver.csproj \
        --configuration Release --runtime "${runtime_id}" --self-contained true \
        --output /out/receiver \
        -p:InformationalVersion="${RPI_OMT_CLIENT_VERSION}" \
        -p:DebugType=None -p:DebugSymbols=false

FROM alpine:3.23.5@${ALPINE_DIGEST} AS python-builder

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

RUN apk add --no-cache build-base python3 python3-dev py3-pip \
    && python3 -m venv /opt/venv
COPY app/requirements.txt /tmp/requirements.txt
RUN /opt/venv/bin/pip install --no-cache-dir --require-hashes \
        -r /tmp/requirements.txt \
    && /opt/venv/bin/pip uninstall -y pip setuptools wheel

FROM alpine:3.23.5@${ALPINE_DIGEST} AS runtime

SHELL ["/bin/ash", "-eo", "pipefail", "-c"]

ARG WEB_PORT=5000
ARG RPI_OMT_CLIENT_VERSION=unknown

RUN apk add --no-cache \
        alsa-lib \
        avahi-libs \
        bash \
        coreutils \
        icu-libs \
        libdrm \
        libgcc \
        libstdc++ \
        openssl \
        python3 \
        util-linux \
        zlib \
    && addgroup -S omt \
    && adduser -S -D -H -G omt -h /etc/omt -s /sbin/nologin omt \
    && mkdir -p /app/legal /etc/omt /usr/local/lib \
    && chown omt:omt /etc/omt

COPY --from=python-builder /opt/venv /opt/venv
COPY --from=receiver-builder /out/receiver/omt-receiver /usr/local/bin/omt-receiver
COPY --from=receiver-builder /out/native/libvmx.so /usr/local/lib/libvmx.so
COPY app/ /app/
COPY omt/ /usr/local/bin/
COPY LICENSE THIRD_PARTY_NOTICES.txt /app/legal/
COPY scripts/generate-runtime-sbom.py /tmp/generate-runtime-sbom.py

RUN chmod 0755 \
        /usr/local/bin/omt-receiver \
        /usr/local/bin/control-omt.sh \
        /usr/local/bin/start-omt.sh \
        /usr/local/bin/entrypoint.sh \
    && { \
        find /opt/venv/lib -path '*dist-info/licenses/*' -type f -print \
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
        --version "${RPI_OMT_CLIENT_VERSION}" \
    && rm /tmp/generate-runtime-sbom.py \
    && printf '%s\n' "${RPI_OMT_CLIENT_VERSION}" > /app/RPI_OMT_CLIENT_VERSION \
    && { \
        find /app -type f ! -name runtime-sha256.manifest -print0; \
        find /usr/local/bin -maxdepth 1 -type f -print0; \
        find /usr/local/lib -maxdepth 1 -type f -name 'libvmx.so' -print0; \
    } | sort -z | xargs -0 sha256sum > /app/runtime-sha256.manifest

ENV HOME=/etc/omt
ENV OMT_CONFIG_DIR=/etc/omt
ENV OMT_STORAGE_PATH=/etc/omt/omt
ENV FLASK_APP=omt_client.wsgi:app
ENV PIP_DISABLE_PIP_VERSION_CHECK=1
ENV PYTHONUNBUFFERED=1
ENV WEB_PORT=${WEB_PORT}
ENV LD_LIBRARY_PATH=/usr/local/lib
ENV PATH="/opt/venv/bin:${PATH}"

LABEL org.opencontainers.image.title="Raspberry Pi OMT Client"
LABEL org.opencontainers.image.version="${RPI_OMT_CLIENT_VERSION}"
LABEL org.opencontainers.image.licenses="LicenseRef-Proprietary"

USER omt
EXPOSE ${WEB_PORT}
VOLUME ["/etc/omt"]
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
