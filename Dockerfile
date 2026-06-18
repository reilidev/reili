# syntax=docker/dockerfile:1.25@sha256:0adf442eae370b6087e08edc7c50b552d80ddf261576f4ebd6421006b2461f12

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
