FROM rust:1.82-slim as builder
WORKDIR /usr/src

RUN apt-get update && \
    apt-get dist-upgrade -y

RUN USER=root cargo new gateway
WORKDIR /usr/src/gateway
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo install --path .


FROM debian:12-slim@sha256:36e591f228bb9b99348f584e83f16e012c33ba5cad44ef5981a1d7c0a93eca22
RUN apt-get -y update && \
    apt-get -y install libssl3 && \
    apt-get clean autoclean && \
    apt-get autoremove --yes && \
    rm -rf /var/lib/{apt,dpkg,cache,log}/
COPY --from=builder /usr/local/cargo/bin/gateway /home/app/gateway
WORKDIR /home/app
USER 1000
CMD ["/home/app/gateway", "/config/runtime_config.yaml"]
