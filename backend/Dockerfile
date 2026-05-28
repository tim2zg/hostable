FROM rust:alpine AS builder

WORKDIR /usr/src/app
# Install dependencies needed for compiling reqwest and xz2
RUN apk add --no-cache musl-dev pkgconfig openssl-dev xz-dev
COPY . .
RUN cargo build --release

FROM alpine:latest
# Install runtime dependencies
RUN apk add --no-cache libgcc openssl xz-libs
COPY --from=builder /usr/src/app/target/release/backend /usr/local/bin/hostable
ENTRYPOINT ["hostable"]
CMD ["manage", "--config", "/config/config.yaml"]
