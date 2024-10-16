FROM rust:1.81 as builder
WORKDIR /usr/src

RUN apt-get update && \
    apt-get dist-upgrade -y

RUN USER=root cargo new gateway
WORKDIR /usr/src/gateway
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo install --path .


FROM debian:12-slim@sha256:ad86386827b083b3d71139050b47ffb32bbd9559ea9b1345a739b14fec2d9ecf
RUN apt-get -y update && \
    apt-get -y install libssl3 && \
    apt-get clean autoclean && \
    apt-get autoremove --yes && \
    rm -rf /var/lib/{apt,dpkg,cache,log}/
COPY --from=builder /usr/local/cargo/bin/gateway /home/app/gateway
WORKDIR /home/app
USER 1000
CMD ["/home/app/gateway", "/config/runtime_config.yaml"]
