# Stage 1: Build Frontend
FROM node:22.21.1-alpine AS frontend-builder
WORKDIR /app
COPY frontend/package*.json ./
RUN npm ci
COPY frontend ./
RUN npm run check && npm run build

# Stage 2: Build Backend
FROM rust:1.88-alpine3.22 AS builder
WORKDIR /usr/src/app/backend
RUN apk add --no-cache build-base musl-dev perl pkgconf openssl-dev xz-dev xz-static
COPY backend .
RUN mkdir -p ../frontend
COPY ansible ../ansible
COPY --from=frontend-builder /app/dist ../frontend/dist
COPY scripts ../scripts
ARG HOSTABLE_BUILD_VERSION=1.5.0
ARG HOSTABLE_BUILD_COMMIT=local
RUN HOSTABLE_BUILD_VERSION="$HOSTABLE_BUILD_VERSION" HOSTABLE_BUILD_COMMIT="$HOSTABLE_BUILD_COMMIT" cargo build --release --locked

# Stage 3: Runtime
FROM alpine:3.22
RUN apk add --no-cache libgcc openssl xz-libs ca-certificates ansible openssh-client postgresql-client gnupg
WORKDIR /etc/hostable
COPY --from=builder /usr/src/app/backend/target/release/backend /usr/local/bin/hostable
ENTRYPOINT ["hostable"]
CMD ["start"]
