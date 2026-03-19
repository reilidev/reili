# syntax=docker/dockerfile:1.7

FROM gcr.io/distroless/static:latest@sha256:28efbe90d0b2f2a3ee465cc5b44f3f2cf5533514cf4d51447a977a5dc8e526d0 AS runtime

ARG APP_VERSION
ARG TARGETARCH

COPY dist/docker/${TARGETARCH}/reili /usr/local/bin/reili

LABEL org.opencontainers.image.title="Reili" \
      org.opencontainers.image.description="Slack-native AI agent for read-only DevOps investigations" \
      org.opencontainers.image.version="${APP_VERSION}" \
      org.opencontainers.image.licenses="Apache-2.0"

USER 10001:10001
WORKDIR /home/reili

ENV PORT=3000

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/reili"]
