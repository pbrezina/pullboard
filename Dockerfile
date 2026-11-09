# Build tools are not application configuration. No credentials enter these stages.
FROM docker.io/library/node:22.23.1-alpine AS frontend
WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM docker.io/library/rust:1.99.1-alpine AS backend
RUN apk add --no-cache build-base cmake pkgconf sqlite-dev
WORKDIR /build
COPY backend/Cargo.toml backend/Cargo.lock ./backend/
COPY backend/src/ ./backend/src/
COPY api/ ./api/
WORKDIR /build/backend
RUN cargo build --release --locked
# Keep runtime dependencies honest: bundled SQLite and Rust TLS need no OpenSSL.
RUN ldd target/release/pullboard || test "$?" -eq 1

FROM docker.io/library/alpine:3.23 AS runtime
RUN apk add --no-cache ca-certificates \
    && addgroup -g 10001 pullboard \
    && adduser -D -H -u 10001 -G pullboard pullboard \
    && mkdir -p /app /data \
    && chown 10001:10001 /data
LABEL org.opencontainers.image.title="PullBoard" \
      org.opencontainers.image.description="Local forge attention board with explicit Jira actions" \
      org.opencontainers.image.licenses="GPL-3.0-or-later"
WORKDIR /app
COPY --from=backend /build/backend/target/release/pullboard /usr/local/bin/pullboard
COPY --from=frontend /build/frontend/dist/frontend/browser/ /app/frontend/
COPY config/container.yaml /app/config.yaml
COPY LICENSE /usr/share/licenses/pullboard/LICENSE
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/pullboard"]
CMD ["--config", "/app/config.yaml"]
