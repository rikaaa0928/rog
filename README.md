# rog

A flexible and powerful network proxy.

## Features

- **Multi-protocol support:** Supports TCP, HTTP, SOCKS5, HTSS5, PB-TCP, gRPC, Reverse gRPC (`rev_grpc`), and Linux `TPROXY` (IPv4/IPv6 dual-stack transparent proxy).
- **Configurable routing:** Flexible routing rules based on CIDR, regex, domain suffix, domain keyword, and dynamic remote rule lists.
- **Asynchronous architecture:** Built with Tokio for high performance and non-blocking operations.
- **Centralized configuration:** Configuration is managed through a TOML file.
- **Memory Management:** Global buffer pool to limit memory usage, supporting percentage-based limits.
- **Independent UDP DNS & IP-Domain Learning:** Built-in dual-stack UDP DNS server with rule-based upstream routing and automatic IP (A/AAAA) → domain mapping for transparent TPROXY routing and cross-stack proxy forwarding.

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

- `endpoint`: The address and port to listen on (e.g., `"0.0.0.0:1080"` or dual-stack `"[::]:12345"`).
- `name`: A unique name for the listener.
- `proto`: The protocol to use (`"tcp"`, `"http"`, `"socks5"`, `"htss5"`, `"pb_tcp"`, `"grpc"`, `"rev_grpc"`, `"tproxy"`).
- `router`: The name of the router to use for this listener.
- `up` / `down`: (Optional) Host commands executed after listener startup and before shutdown (commonly used with `tproxy` to load/unload `iptables`/`ip6tables` and policy routing rules).
- `options`: Optional protocol parameters.
  - `tproxy` supports `mode` (`"tcp+udp"` by default, `"tcp"`, `"udp"`), `udp_idle_timeout_secs`, and `udp_max_senders`.
  - `grpc`/`rev_grpc` support `keep_alive`, `keep_alive_interval_secs`, `keep_alive_timeout_secs`, `keep_alive_while_idle`, `connect_timeout_secs`, `tcp_nodelay`, `tcp_keepalive_secs`, `tcp_keepalive_interval_secs`, `tcp_keepalive_retries`, `initial_stream_window_size`, `initial_connection_window_size`, `stream_channel_size`, `channel_buffer_size`, `max_decoding_message_size`, `max_encoding_message_size`, `max_concurrent_streams`, `max_frame_size`, `concurrency_limit`, `concurrency_limit_per_connection`, and related gRPC transport settings.

#### `router`

- `name`: A unique name for the router.
- `default`: The default connector name to use.
- `route_rules`: A list of routing rules.

#### `router.route_rules`

- `name`: Name of the `data` source to match.
- `select`: Name of the `connector` to use on match.
- `exclude`: A list of `data` source names to exclude.
- `domain_to_ip`: (Optional) Whether to resolve the domain to an IP address before matching CIDR rules.
- `dns`: (Optional) Specific UDP DNS server to use when `domain_to_ip = true` (e.g., `"223.5.5.5:53"`).

#### `data`

- `name`: A unique name for the data source.
- `url`: An optional URL to load data from (`http://`, `https://`, `file://`, with optional `#interval=3600` fragment for refresh interval in seconds).
- `format`: The format of the data (`"cidr"`, `"regex"`, `"lan"`, `"domain-suffix"` / `"suffix"`, `"domain-keyword"` / `"keyword"`).
- `interval`: (Optional) Periodic refresh interval in seconds for `url` sources (default `3600`, overridden by `#interval=<secs>` if present in `url`). Non-blocking background refresh.
- `data`: Inline multi-line data.

#### `connector`

- `endpoint`: The endpoint of the connector.
- `name`: A unique name for the connector.
- `user`: Optional username for authentication.
- `pw`: Optional password for authentication.
- `proto`: The protocol of the connector (`"tcp"`, `"socks5"`, `"http"`, `"htss5"`, `"pb_tcp"`, `"grpc"`, `"rev_grpc"`, `"block"`).
- `dns`: (Optional) Upstream UDP DNS server list associated with this connector (e.g., `["114.114.114.114:53", "223.5.5.5:53"]`). Falls back to `dns.default_servers` when omitted.
- `options`: Optional protocol parameters.
  - `bootstrap_dns`: (Optional) Direct UDP DNS servers used to resolve the proxy connector's own domain endpoint. Falls back to `connector.dns`, then `dns.default_servers` when omitted.
  - `grpc` supports the same gRPC transport settings as listeners; see `full_config_example.toml` for recommended values.

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

## Independent UDP DNS Service

Add a `[dns]` section to enable the built-in UDP DNS server (supports dual-stack `"[::]:53"`). It reuses the routing rules from `dns.router` (skipping IP-only rules) to route DNS queries and learns real IP (A/AAAA) → domain mappings in `IpDomainTable`:
- When a matched connector defines a custom `dns` list (such as `Direct` with domestic DNS `["114.114.114.114:53", "223.5.5.5:53"]`), queries are sent via that connector to its `dns` servers.
- Otherwise, queries use `dns.default_servers` via the router's default outbound connector.
- Set `disable_aaaa = true` to return empty `NOERROR` responses for `AAAA` queries (defaults to `false`).
- When paired with a `tproxy` listener sharing the same `router` in the same process, `rog` automatically maps destination IPs back to learned domain names for domain-based routing, and rewrites IPv6 target addresses back to domain names when forwarding through non-direct proxy connectors.

## DNS Mapping Observation

Set `PROXY_OBSERVE_LISTEN=127.0.0.1:18080` (and optional `PROXY_OBSERVE_TOKEN`) to enable the Observe HTTP API. `GET /api/v1/observe/ip-domains` returns active IP → domain mappings with filtering (`router`, `client_ip`, `target_ip`, `domain`, `search`) and pagination (`limit`, `offset`).

## Contributing

Contributions are welcome! Please feel free to submit pull requests or open issues.

## License

This project is licensed under the [LICENSE NAME] license.
