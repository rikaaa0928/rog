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
  - `scripts/tproxy-up.sh` and `scripts/tproxy-down.sh` are **universal across machines by default**: they auto-detect the primary LAN interface (`ROG_IFACE`), local IPv4 address (`ROG_IPV4`), local IPv4 subnet (`ROG_LAN_CIDR`), and on-link IPv6 RA prefixes, and configure `sysctl`, policy routing, dual-stack `mangle` TPROXY chains, UDP `:53` DNS redirection, and `MASQUERADE`.
  - Optional positional arguments (`[TPROXY_PORT] [FWMARK] [TABLE_ID] [MODE] [FAMILY]`) or environment variables can be used to override defaults:
    - `ROG_IFACE`: LAN interface name (auto-detected from default route)
    - `ROG_IPV4`: Local IPv4 address on `ROG_IFACE` (auto-detected)
    - `ROG_LAN_CIDR`: Local IPv4 subnet CIDR on `ROG_IFACE` (auto-detected)
    - `ROG_DNS_PORT`: Local UDP DNS port to redirect port 53 to (default `53`, `0` to disable)
    - `ROG_USER` / `ROG_GROUP`: Service user/group bypassed in `OUTPUT` chains (default `rog`)
    - `ROG_BYPASS_IPV4`: Space-separated IPv4 CIDRs to bypass TPROXY (optional)
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

## TPROXY Transparent Proxy & Side-Gateway Guide (Linux Dual-Stack)

`rog` supports building a Linux transparent proxy or LAN side-gateway by combining the **`tproxy` listener** with the **built-in `[dns]` UDP server**, supporting **IPv4/IPv6 dual-stack** and **full TCP/UDP interception**.

### 1. How It Works

Unlike TUN + Fake-IP setups, `rog`'s `TPROXY + Real DNS` mode requires no virtual TUN device and returns real public IP addresses to clients:
1. **DNS Routing & IP-Domain Learning**: Clients point their DNS to the gateway (or have port 53 transparently redirected by `nat` rules to `rog`'s `[::]:53`). `rog` routes each DNS query using `router` domain rules, returns the real `A`/`AAAA` records to the client, and records `target_ip -> domain` in `IpDomainTable`.
2. **Transparent TCP/UDP Interception**: When clients connect to those real IPs over TCP or UDP (e.g., QUIC, NTP), Linux `TPROXY` delivers the traffic to `rog` (`[::]:12345`) while preserving the original destination IP/port. `rog` looks up the destination IP in `IpDomainTable` to recover the domain name and evaluate domain routing rules.
3. **Cross-Stack Proxy Forwarding (IPv6 Client $\rightarrow$ IPv4-Only Proxy Outbound)**: When an IPv6 client connects to a learned domain via a non-direct proxy connector (such as a remote VPS with IPv4-only egress), `rog` automatically rewrites the destination back to `domain:port` so the remote proxy node resolves and connects over IPv4.

### 2. Complete Side-Gateway Configuration Example (`/etc/rog/config.toml`)

```toml
[dns]
endpoint = "[::]:53"
router = "main"
default_servers = ["1.1.1.1:53", "8.8.8.8:53"]
disable_aaaa = false # Set to true to strip AAAA responses if clients only configure an IPv4 gateway
upstream_timeout_ms = 2000
request_timeout_ms = 8000

[[listener]]
name = "tproxy_inbound"
endpoint = "[::]:12345"
proto = "tproxy"
router = "main"
# Can be managed via listener up/down hooks or systemd ExecStartPre/ExecStopPost:
# up = "/etc/rog/tproxy-up.sh"
# down = "/etc/rog/tproxy-down.sh"
[listener.options]
mode = "tcp+udp"
udp_idle_timeout_secs = 60
udp_max_senders = 512

[[connector]]
name = "Proxy"
proto = "grpc"
endpoint = "https://proxy.example.com:443"
pw = "your_password"
# Inherits dns and bootstrap_dns from [dns].default_servers automatically
[connector.options]
connect_timeout_secs = 5
keep_alive = true
keep_alive_interval_secs = 30
keep_alive_timeout_secs = 20
keep_alive_while_idle = true
tcp_keepalive_secs = 30

[[connector]]
name = "Direct"
proto = "tcp"
dns = ["114.114.114.114:53", "223.5.5.5:53"]

[[data]]
name = "lan_and_cn_cidr"
format = "cidr"
data = """
127.0.0.0/8
10.0.0.0/8
172.16.0.0/12
192.168.0.0/16
::1/128
fc00::/7
fe80::/10
"""

[[data]]
name = "cn_domains"
format = "domain-suffix"
url = "https://example.com/cn-domain-suffix.txt#interval=3600"

[[router]]
name = "main"
default = "Proxy"

[[router.route_rules]]
name = "lan_and_cn_cidr"
select = "Direct"
exclude = []

[[router.route_rules]]
name = "cn_domains"
select = "Direct"
exclude = []
```

### 3. Systemd Deployment (`rog.service`)

Because `scripts/tproxy-up.sh` intercepts both forwarded LAN traffic (`PREROUTING`) and local host traffic (`OUTPUT`), **run `rog` under a dedicated user/group (`rog:rog`)** so `ROG_OUT` can bypass `rog`'s own outbound sockets via `-m owner --uid-owner` / `--gid-owner` and prevent routing loops:

```bash
ADMIN_USER="$(id -un)"
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin rog || true
sudo install -d -o "$ADMIN_USER" -g rog -m 2775 /etc/rog
sudo install -o root -g root -m 0755 target/release/rog /usr/local/bin/rog
install -m 0775 scripts/tproxy-up.sh /etc/rog/tproxy-up.sh
install -m 0775 scripts/tproxy-down.sh /etc/rog/tproxy-down.sh
```

Create `/etc/systemd/system/rog.service`:

```ini
[Unit]
Description=Rog TPROXY and UDP DNS Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=rog
Group=rog
Environment=ROG_CONFIG=/etc/rog/config.toml
Environment=PROXY_OBSERVE_LISTEN=0.0.0.0:19090
Environment=PROXY_OBSERVE_TOKEN=token
AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE
NoNewPrivileges=true
ExecStartPre=+/etc/rog/tproxy-up.sh
ExecStart=/usr/local/bin/rog
ExecStopPost=+/etc/rog/tproxy-down.sh
Restart=on-failure
RestartSec=3s

[Install]
WantedBy=multi-user.target
```

### 4. LAN Client Configuration

- **IPv4**: Point the client's **Default Gateway (Router)** and **DNS Server** to the side-gateway's IPv4 address.
- **IPv6**:
  - **Option A (Full Dual-Stack IPv6 Proxy)**: Point the client's **IPv6 Router** to the side-gateway's link-local address (`fe80::...` under `scope link` on the LAN interface).
  - **Option B (IPv4 Gateway Only)**: Set `disable_aaaa = true` under `[dns]` so clients only receive `A` records and route all domain traffic through the IPv4 side-gateway.

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
