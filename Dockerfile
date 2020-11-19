FROM registry.gitlab.com/osrdata/gateway:base_master as builder

FROM scratch
COPY --from=builder /usr/local/cargo/bin/gateway .
USER 1000
CMD ["./gateway"]
