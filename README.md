# gateway

`gateway` is a simple API gateway written in Rust.

Simple use: `cargo run -- local_config.yml`.

## Configuration

The configuration files must contain the following keys:

```yaml
bind_to: # (Mandatory) the `SocketAddr` to listen
crd_label: # TODO
metrics_prefix: gateway_dev
perm_uris: [] # endpoints where to fetch premissions
perm_update_delay: 30 # delay between each permissions update, in seconds
auth_sources: [] # TODO
max_fetch_error_count: u64 # max number of consecutive errors when fetching permissions

# TODO: arbitrary values
websocket_config:
  write_buffer_size: 10_000
  # This must at least be write_buffer_size + 1.
  # See https://docs.rs/tungstenite/0.24.0/tungstenite/protocol/struct.WebSocketConfig.html#structfield.max_write_buffer_size
  max_write_buffer_size: 1_000_000
  max_message_size: 1_000_000
  max_frame_size: 1_000_000
  accept_unmasked_frames: true

# Optional config to fetch on namespaces `test` and `test2` only.
# crds_namespaces:
#  - test
#  - test2
```

## Optional features

- `remove_authorization_header` — Remove the header `Authorization` from the
  forwarded request

## TODO

- Add chain request/response logic
