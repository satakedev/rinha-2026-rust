# syntax=docker/dockerfile:1.7

# Stage 1: build the api and build-dataset binaries.
# api: target-cpu=x86-64-v3 unlocks AVX2/FMA on the Mac Mini Late 2014
# (Haswell) environment used by the Rinha test engine — required by the
# AVX2 scan kernel.
# build-dataset: built with the default target-cpu (x86-64). It is a
# one-shot build-time tool with no SIMD hot path, and v3 autovectorization
# triggers SIGSEGV under Rosetta 2 on Apple Silicon dev hosts (sub-task 3.3
# requires the cross-arch buildx flow to succeed there).
FROM --platform=linux/amd64 rust:1.85-bookworm AS builder
ENV CARGO_TERM_COLOR=never \
    CARGO_NET_RETRY=10
WORKDIR /work
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/work/target,sharing=locked \
    cargo build --release --bin build-dataset \
 && mkdir -p /out \
 && cp target/release/build-dataset /out/build-dataset \
 && RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release --bin api \
 && cp target/release/api /out/api

# Stage 2: run build-dataset to materialize references.i8.bin + labels.bits.
# Needs glibc + a writable filesystem; debian:bookworm-slim is the smallest
# base that pairs cleanly with the rust:bookworm builder.
FROM --platform=linux/amd64 debian:bookworm-slim AS dataset
WORKDIR /work
COPY --from=builder /out/build-dataset /usr/local/bin/build-dataset
COPY resources/references.json.gz ./resources/references.json.gz
RUN mkdir -p /dataset \
 && /usr/local/bin/build-dataset resources/references.json.gz /dataset \
 && ls -la /dataset

# Stage 3: minimal runtime. distroless/cc-debian12 ships glibc + libgcc and
# nothing else (no shell, no package manager). The dataset volume is mounted
# at runtime by docker-compose; resources JSONs are baked into the image.
FROM --platform=linux/amd64 gcr.io/distroless/cc-debian12 AS runtime
COPY --from=builder /out/api /usr/local/bin/api
COPY resources/normalization.json /etc/rinha/normalization.json
COPY resources/mcc_risk.json /etc/rinha/mcc_risk.json
ENV RINHA_REFS=/var/lib/rinha/dataset/references.i8.bin \
    RINHA_LABELS=/var/lib/rinha/dataset/labels.bits \
    RINHA_NORMALIZATION=/etc/rinha/normalization.json \
    RINHA_MCC_RISK=/etc/rinha/mcc_risk.json \
    RINHA_BIND=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/api"]
