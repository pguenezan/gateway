# gateway

`gateway` is a simple API gateway written in Rust. It check and compile his
config file (`gateway/src/config.rs`) at compile time. You also need to add the
public key for the JWT token in `gateway/src/public_key.pem`.

## Environment

| Variable    | Description                                 |
|-------------|---------------------------------------------|
| `BIND_TO`   | *Mandatory* the `SocketAddr` to listen      |
| `JWT_ISSER` | Check if the JWT token has the right issuer |

## Optional features

* `remove_authorization_header` — Remove the header `Authorization` from the
  forwarded request
