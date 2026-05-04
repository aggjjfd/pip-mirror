# syntax=docker/dockerfile:1

# ---- builder ----
FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
RUN cargo build --release --target x86_64-unknown-linux-musl
RUN strip target/x86_64-unknown-linux-musl/release/pip-mirror

# ---- runtime ----
FROM scratch
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/pip-mirror /pip-mirror
VOLUME ["/repo/packages"]
EXPOSE 8080
ENTRYPOINT ["/pip-mirror"]
CMD ["serve", "--host", "0.0.0.0", "--port", "8080"]
