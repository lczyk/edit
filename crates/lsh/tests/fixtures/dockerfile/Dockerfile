# Build stage
FROM ubuntu:24.04 AS builder

ARG VERSION=1.0
ENV APP_HOME=/srv/app
WORKDIR ${APP_HOME}

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY . .
RUN ./build.sh

# Runtime stage
FROM debian:bookworm-slim
LABEL org.opencontainers.image.version="${VERSION}"
EXPOSE 8080
USER 1000:1000
HEALTHCHECK --interval=30s CMD curl -f http://localhost:8080/ || exit 1
ENTRYPOINT ["/srv/app/run"]
CMD ["--serve"]

# A trailing backslash continues the instruction onto the next line.
ENV FOO=bar \
    BAZ=qux
RUN mkdir -p /opt/app && \
    ENV=not-an-instruction cp -r src /opt/app \
    # a comment inside a continued RUN
    && chmod 755 /opt/app
LABEL org.opencontainers.image.title="edit" \
      org.opencontainers.image.source="https://example.invalid"
