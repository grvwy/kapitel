# syntax=docker/dockerfile:1
FROM rust:1.97.0-slim-trixie@sha256:686a437ead83701e8f871e66e838c3ec55f46b5fc235b025756396ac823bdc51 AS development
WORKDIR /workspace
RUN rustup component add rustfmt clippy
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN mkdir src && printf 'fn main() {}\n' > src/main.rs && cargo fetch --locked && rm -r src
COPY src ./src
COPY web ./web
COPY tests ./tests

FROM development AS test
RUN --mount=type=cache,id=kapitel-target,target=/workspace/target \
    cargo fmt --all --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked

FROM rust:1.97.0-slim-trixie@sha256:686a437ead83701e8f871e66e838c3ec55f46b5fc235b025756396ac823bdc51 AS audit-tools
RUN cargo install cargo-audit --locked --version 0.22.2

FROM audit-tools AS audit
WORKDIR /workspace
COPY Cargo.lock ./
COPY .cargo/audit.toml .cargo/audit.toml
RUN cargo audit --deny warnings

FROM development AS build
RUN --mount=type=cache,id=kapitel-target,target=/workspace/target \
    cargo build --locked --release && cp target/release/kapitel /kapitel

FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f AS runtime
LABEL org.opencontainers.image.source="https://github.com/grvwy/kapitel" \
      org.opencontainers.image.description="A small self-hosted audiobook player that remembers your place" \
      org.opencontainers.image.licenses="MIT"
RUN groupadd --gid 10001 kapitel && useradd --uid 10001 --gid 10001 --no-create-home kapitel \
    && mkdir /library /data && chown 10001:10001 /data
COPY --from=build /kapitel /usr/local/bin/kapitel
USER 10001:10001
ENV KAPITEL_LIBRARY_DIRECTORY=/library KAPITEL_DATA_DIRECTORY=/data
EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["kapitel"]
CMD ["serve"]
