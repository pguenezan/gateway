FROM rustlang/rust:nightly

WORKDIR /usr/src/gateway
COPY . .

RUN cargo install --path gateway
RUN rm -rf /usr/src/gateway

CMD ["gateway"]
