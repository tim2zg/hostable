# Stage 1: Build Frontend
FROM node:20-alpine AS frontend-builder
WORKDIR /app
COPY frontend/package*.json ./
RUN npm install
COPY frontend ./
RUN npm run build

# Stage 2: Build Backend
FROM rust:alpine AS builder
WORKDIR /usr/src/app/backend
RUN apk add --no-cache musl-dev pkgconfig openssl-dev xz-dev xz-static
COPY backend .
RUN mkdir -p ../frontend
COPY --from=frontend-builder /app/dist ../frontend/dist
RUN cargo build --release

# Stage 3: Runtime
FROM alpine:latest
RUN apk add --no-cache libgcc openssl xz-libs
COPY --from=builder /usr/src/app/backend/target/release/backend /usr/local/bin/hostable
ENTRYPOINT ["hostable"]
CMD ["manage", "--config", "/config/config.yaml"]
