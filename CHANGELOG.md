# 2.0.0

- Option `websocket_config.max_send_queue` was replaced by
  `websocket_config.write_buffer_size` and
  `websocket_config.max_write_buffer_size`
- Target kube API version 1.31
- Upgrade to hyper 1.0
- Upgrade all dependencies, base image and to Rust 1.81
