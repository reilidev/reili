# syntax=docker/dockerfile:1.28@sha256:bb22d9815c728170f72750f4e5b0d672e06176142e1d602c7e66c050100b7e5b

FROM gcr.io/distroless/static:latest@sha256:f2ea2709ac8db56323cbd7d014277f32cb572d9ea124b0076f7aafe5980678fe AS runtime

ARG APP_VERSION
ARG TARGETARCH

COPY dist/docker/${TARGETARCH}/reili /usr/local/bin/reili

LABEL org.opencontainers.image.title="Reili" \
      org.opencontainers.image.version="${APP_VERSION}" \
      org.opencontainers.image.licenses="Apache-2.0"

USER 10001:10001
WORKDIR /home/reili

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/reili"]
