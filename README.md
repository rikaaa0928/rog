# rog

A flexible and powerful network proxy.

## Features

- **Multi-protocol support:** Supports various protocols including TCP, HTTP, SOCKS5, gRPC, pb_tcp, and pb_http(HTTP/2/HTTP/3 + protobuf).
- **Configurable routing:** Allows flexible routing rules based on various criteria.
- **Asynchronous architecture:** Built with Tokio for high performance and non-blocking operations.
- **Centralized configuration:** Configuration is managed through a TOML file.
- **Memory Management:** Global buffer pool to limit memory usage, supporting percentage-based limits.
- **Smart DNS:** Per-rule DNS server configuration for flexible name resolution.

## Getting Started

### Prerequisites

- Rust (latest stable version)
- Cargo

### Building

```bash
cargo build --release
```

### Running

By default, rog looks for a configuration file at `/etc/rog/config.toml`. You can specify a different path using the `ROG_CONFIG` environment variable:

```bash
export ROG_CONFIG=/path/to/your/config.toml
./target/release/rog
```

### Configuration

The configuration is managed through a TOML file.

#### Global Configuration

- `server_id`: (Optional) Unique identifier for this proxy instance.
- `buffer_size`: (Optional) Global TCP buffer limit. Supports bytes (e.g., "64MB"), percentage of system memory (e.g., "50%"), or "off" to disable.

Here's an example structure:

```toml
# Global settings
server_id = "my-proxy-01"
buffer_size = "50%" # Use 50% of available memory for TCP buffers

[[listener]]
endpoint = "0.0.0.0:1080"
name = "socks5_listener"
proto = "socks5"
router = "default"

[[router]]
name = "default"
default = "direct"

[[router.route_rules]]
name = "rule1"
select = "example.com"

[[data]]
name = "direct"
format = "direct"

[[connector]]
name = "direct"
proto = "tcp"
```

#### `listener`

- `endpoint`: The address and port to listen on.
- `name`: A unique name for the listener.
- `proto`: The protocol to use (e.g., "tcp", "http", "socks5", "grpc", "pb_tcp", "pb_http").
- `router`: The name of the router to use for this listener.

#### `router`

- `name`: A unique name for the router.
- `default`: The default route to use.
- `route_rules`: A list of routing rules.

#### `router.route_rules`

- `name`: A unique name for the rule.
- `select`: The pattern to match against.
- `exclude`: A list of patterns to exclude.
- `domain_to_ip`: Whether to resolve the domain to an IP address.
- `dns`: (Optional) Specific DNS server to use for this rule (e.g., "8.8.8.8:53").

#### `data`

- `name`: A unique name for the data source.
- `url`: An optional URL to load data from.
- `format`: The format of the data.
- `data`: Inline data.

#### `connector`

- `endpoint`: The endpoint of the connector.
- `name`: A unique name for the connector.
- `user`: Optional username for authentication.
- `pw`: Optional password for authentication.
- `proto`: The protocol of the connector (e.g., "tcp", "grpc", "pb_tcp", "pb_http").

#### `pb_http`

`pb_http` carries protobuf messages over HTTP and keeps the same authentication and payload encryption semantics as `pb_tcp`. The rog server listens for cleartext HTTP/2 only (`h2c`) so it can sit behind a proxy that terminates TLS, HTTP/2, and HTTP/3. The client supports `h2c`, TLS HTTP/2 (`h2`), and HTTP/3 (`h3`); by default it tries `h3` then falls back to `h2`.

Common options are `path` (default `pb_http`, producing `/<path>/stream` and `/<path>/udp`), `transport` (`h2c`, `h2`, or `h3` to force one client transport), `fallback_order` (for example `["h3", "h2"]`), `tls_server_name`, `tls_ca_cert_file`, and `tls_insecure_skip_verify`. TLS uses WebPKI roots by default; no certificate file is required for public CA certificates.

Performance-related options include `h2_initial_stream_window_size`, `h2_initial_connection_window_size`, `h2_max_frame_size`, `h2_max_header_list_size`, `h2_max_concurrent_streams`, `h2_header_table_size`, `h2_max_send_buffer_size`, `h2_max_concurrent_reset_streams`, `h2_max_pending_accept_reset_streams`, `h2_reset_stream_duration_ms`, `h3_max_field_section_size`, `h3_send_grease`, `h3_bind`, `quic_max_idle_timeout_ms`, `quic_keep_alive_interval_ms`, `quic_initial_rtt_ms`, `quic_stream_receive_window`, `quic_receive_window`, `quic_send_window`, `quic_max_concurrent_bidi_streams`, `quic_max_concurrent_uni_streams`, `quic_send_fairness`, `send_chunk_size`, and `max_message_size`. Server listeners also support `accept_channel_size`.

## Usage

Here's an example of how to configure rog to act as a SOCKS5 proxy:

**1. Create a `config.toml` file:**

```toml
[[listener]]
endpoint = "0.0.0.0:1080"
name = "socks5_listener"
proto = "socks5"
router = "default"

[[router]]
name = "default"
default = "proxy_connector"

[[router.route_rules]]
name = "rule1"
select = "*.example.com"
connector = "proxy_connector"

[[connector]]
name = "proxy_connector"
proto = "socks5"
endpoint = "127.0.0.1:9050" # Replace with your actual upstream SOCKS5 proxy
```

This configuration sets up a SOCKS5 listener on `0.0.0.0:1080`. All traffic will be routed using the `default` router. The `default` router has a rule that forwards traffic to `*.example.com` to the `proxy_connector`. The `proxy_connector` is configured as a SOCKS5 client connecting to `127.0.0.1:9050`.

**2. Run the proxy:**

```bash
./target/release/rog
```

Now, you can configure your applications to use `localhost:1080` as a SOCKS5 proxy. Any requests to domains ending with `example.com` will be forwarded to the upstream proxy at `127.0.0.1:9050`.

**Note:** Replace `127.0.0.1:9050` with the actual address of your upstream SOCKS5 proxy server.

## Contributing

Contributions are welcome! Please feel free to submit pull requests or open issues.

## License

This project is licensed under the [LICENSE NAME] license.
