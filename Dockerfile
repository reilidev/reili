# syntax=docker/dockerfile:1.23@sha256:2780b5c3bab67f1f76c781860de469442999ed1a0d7992a5efdf2cffc0e3d769

FROM gcr.io/distroless/static:latest@sha256:3592aa8171c77482f62bbc4164e6a2d141c6122554ace66e5cc910cadb961ff0 AS runtime

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
