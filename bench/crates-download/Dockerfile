# The installer: Rust 1.88 (Zit's minimum) plus curl for the crate tarball.
FROM rust:1.88-slim
RUN apt-get update -q && apt-get install -y -q --no-install-recommends curl ca-certificates \
  && rm -rf /var/lib/apt/lists/*
