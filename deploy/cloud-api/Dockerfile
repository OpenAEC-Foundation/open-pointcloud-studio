# syntax=docker/dockerfile:1.27@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
# The cloud API of Open Pointcloud Studio: open-pointcloud-studio-api.
#   docker build -f deploy/cloud-api/Dockerfile -t open-pointcloud-studio-api .
#   docker run --rm -p 127.0.0.1:8080:8080 -e OPENAEC_API_TOEGANG=open open-pointcloud-studio-api
# In production OPENAEC_API_SLEUTELS instead (docs/cloud-api.md).
# The images are pinned by digest; the tag says the version. To update:
# docker buildx imagetools inspect <tag>, then replace tag and digest together.
ARG RUST_IMAGE=rust:1.99-trixie@sha256:931c2c0531a8b143e044f0f3914dfe802f15bb3d1955f0b657e9359327ac2ffc
ARG RUNTIME_IMAGE=debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad

FROM ${RUST_IMAGE} AS build
# The number of compile jobs; without it Cargo takes every processor.
ARG CARGO_JOBS
RUN apt-get update && apt-get install -y --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*
WORKDIR /source
COPY native ./native
# The notices are written without the network, after the build: they name
# only the crates the build downloaded for this target.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/source/native/target,sharing=locked \
    cd native \
 && cargo build --release --locked -p open-pointcloud-studio-api ${CARGO_JOBS:+-j "$CARGO_JOBS"} \
 && install -D target/release/open-pointcloud-studio-api /out/open-pointcloud-studio-api \
 && CARGO_NET_OFFLINE=true python3 -I tools/third-party-notices.py --manifest-path Cargo.toml \
      --package open-pointcloud-studio-api --target "$(rustc -vV | sed -n 's/^host: //p')" \
      --output /out/THIRD-PARTY-NOTICES.txt

FROM ${RUNTIME_IMAGE}
ARG VERSION=dev
ARG REVISION=unknown
LABEL org.opencontainers.image.title="Open Pointcloud Studio API" \
      org.opencontainers.image.description="Point-cloud conversion, section drawings and meshes over HTTP and MCP" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.source="https://github.com/OpenAEC-Foundation/open-pointcloud-studio" \
      org.opencontainers.image.documentation="https://github.com/OpenAEC-Foundation/open-pointcloud-studio/blob/main/docs/cloud-api.md" \
      org.opencontainers.image.licenses="LGPL-3.0-or-later" \
      org.opencontainers.image.vendor="OpenAEC Foundation"
RUN groupadd --system --gid 10001 api \
 && useradd --system --uid 10001 --gid 10001 --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin api \
 && install -d -o 10001 -g 10001 /work /work/tmp
# Declared so that a bind-mounted volume at /work (Fargate) takes the owner of
# the image's folder; the service checks at start that it can write there.
VOLUME ["/work"]
COPY --from=build /out/open-pointcloud-studio-api /usr/local/bin/
COPY --from=build /out/THIRD-PARTY-NOTICES.txt /usr/share/doc/open-pointcloud-studio-api/
COPY LICENSE.md /usr/share/doc/open-pointcloud-studio-api/LICENSE-LGPL-3.0.md
COPY native/desktop/LICENSE-GPL-3.0 /usr/share/doc/open-pointcloud-studio-api/LICENSE-GPL-3.0.txt
ENV OPENAEC_MCP_HTTP=0.0.0.0:8080 OPENAEC_MCP_LOG=info OPENAEC_API_LOGVORM=json \
    OPENAEC_API_WERKMAP=/work TMPDIR=/work/tmp
EXPOSE 8080
USER 10001:10001
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --start-interval=2s --retries=5 \
  CMD ["/usr/local/bin/open-pointcloud-studio-api", "--health-check"]
ENTRYPOINT ["/usr/local/bin/open-pointcloud-studio-api"]
