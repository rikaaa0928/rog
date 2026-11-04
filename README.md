# rog

A flexible and powerful network proxy.

## Features

- **Multi-protocol support:** Supports TCP, HTTP, SOCKS5, HTSS5, PB-TCP, gRPC, Reverse gRPC (`rev_grpc`), and Linux `TPROXY` (IPv4/IPv6 dual-stack transparent proxy).
- **Configurable routing:** Flexible routing rules based on CIDR, regex, domain suffix, domain keyword, and dynamic remote rule lists.
- **Asynchronous architecture:** Built with Tokio for high performance and non-blocking operations.
- **Centralized configuration:** Configuration is managed through a TOML file.
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

Here's an example structure:

```toml
# Global settings
server_id = "my-proxy-01"

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

Unlike TUN + Fake-IP setups, `rog`'s `TPROXY + Real DNS + eBPF Direct Fast-Path` mode requires no virtual TUN device and returns real public IP addresses to clients:
1. **DNS Routing & IP-Domain Learning**: Clients point their DNS to the gateway (or have port 53 transparently redirected by `nat` rules to `rog`'s `[::]:53`). `rog` routes each DNS query using `router` domain rules, returns the real `A`/`AAAA` records to the client, and records `target_ip -> domain` in `IpDomainTable`.
2. **eBPF TC Ingress Direct Fast-Path Offload (`ebpf_direct_offload = true`, enabled by default)**:
   - On startup, `rog` uses pure-Rust `SYS_bpf` and `AF_NETLINK` (with zero external `clang`/`llvm`/`tc` dependencies) to attach `rog_direct_tc` (`BPF_PROG_TYPE_SCHED_CLS`) to the LAN interface's `tc ingress` hook, maintaining 4 kernel BPF maps: `v4_lpm` / `v6_lpm` (`BPF_MAP_TYPE_LPM_TRIE`, synced with static and remote `Direct` CIDRs) and `v4_lru` / `v6_lru` (`BPF_MAP_TYPE_LRU_HASH`, updated in real time with `CLOCK_MONOTONIC` TTL expiration whenever `DnsServer` resolves a `Direct` domain, and automatically invalidated if a shared CDN IP is later resolved for a proxy domain).
   - Packets destined for `Direct` IPs are marked `0x78` at `tc ingress`, match `-m mark --mark 0x78/0xff -j RETURN` in `ROG_TP`/`ROG6_TP`, and are forwarded 100% by the Linux kernel at L3 with zero userspace context switches or memory copies.
3. **Transparent TCP/UDP Interception**: Proxy traffic is intercepted by Linux `TPROXY` (`[::]:12345`) while preserving the original destination IP/port. `rog` looks up the destination IP in `IpDomainTable` to recover the domain name and evaluate domain routing rules.
4. **Cross-Stack Proxy Forwarding (IPv6 Client $\rightarrow$ IPv4-Only Proxy Outbound)**: When an IPv6 client connects to a learned domain via a non-direct proxy connector (such as a remote VPS with IPv4-only egress), `rog` automatically rewrites the destination back to `domain:port` so the remote proxy node resolves and connects over IPv4.
5. **Automatic L2-Unicast IPv6 RA / RDNSS Suppression (`suppress_ipv6_ra = true`, enabled by default)**:
   - On LANs where the main router broadcasts IPv6 RAs with `RDNSS`, devices like Android / Chromecast / Google TV forcibly merge the main router's IPv6 DNS and IPv6 gateway even when configured with a static IPv4 gateway/DNS.
   - `rog` automatically detects active side-gateway clients from DNS and TPROXY traffic, resolves their MAC addresses from `/proc/net/arp`, and sends L2-unicast zero-lifetime (`lifetime = 0`) RA counter-frames directly to those client MACs (without affecting other LAN devices). Clients inactive for more than `suppress_ipv6_ra_ttl_secs` (default `300` seconds) are automatically removed from the suppression list.

### 2. Complete Side-Gateway Configuration Example (`/etc/rog/config.toml`)

```toml
[dns]
endpoint = "[::]:53"
router = "main"
default_servers = ["1.1.1.1:53", "8.8.8.8:53"]
bootstrap_servers = ["114.114.114.114:53", "223.5.5.5:53"]
disable_aaaa = false # Set to true to strip AAAA responses if desired
suppress_ipv6_ra = true # Default true: L2-unicast zero-lifetime RA/RDNSS suppression for active clients
suppress_ipv6_ra_ttl_secs = 300 # Inactivity expiration in seconds for active client RA suppression (default 300)
ebpf_direct_offload = true # Default true: eBPF TC ingress kernel fast-path for Direct traffic
ebpf_direct_connectors = ["Direct"] # Optional: connector names offloaded by eBPF; defaults to all proto = "tcp" connectors when omitted
upstream_timeout_ms = 2000
request_timeout_ms = 8000

[[listener]]
name = "tproxy_inbound"
endpoint = "[::]:12345"
proto = "tproxy"
router = "main"
[listener.options]
mode = "tcp+udp"
udp_idle_timeout_secs = 60
udp_max_senders = 512

[[connector]]
name = "Proxy"
proto = "h3"
endpoint = "https://proxy.example.com:443"
pw = "your_password"
# Inherits dns and bootstrap_dns from [dns].default_servers / bootstrap_servers automatically
[connector.options]
connect_timeout_secs = 5
keep_alive = true
keep_alive_interval_secs = 25
max_idle_timeout_secs = 90
connections = 2
fallback_h2 = true
tcp_nodelay = true
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

### 3. Dedicated User/Group Setup & Systemd Deployment (`rog.service`)

#### 3.1 Why a Dedicated `rog:rog` User & Group Is Required
Because `scripts/tproxy-up.sh` intercepts both forwarded LAN traffic (`PREROUTING`) and local host traffic (`OUTPUT`), running `rog` as `root` or an unexempted user would cause `rog`'s own outbound proxy connections and DNS queries to be re-intercepted by `OUTPUT`, creating an infinite routing loop. Running `rog` under a dedicated `rog:rog` user/group allows `ROG_OUT` and `ROG_DNS_OUT` to bypass `rog` traffic via `-m owner --uid-owner rog` and `-m owner --gid-owner rog`.

#### 3.2 Create the `rog` User/Group and Configuration Directory
```bash
# 1. Create dedicated system user and group 'rog'
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin rog || true

# 2. Create /etc/rog owned by current admin user and group 'rog' with SGID (2775)
ADMIN_USER="$(id -un)"
sudo install -d -o "$ADMIN_USER" -g rog -m 2775 /etc/rog

# 3. Install binary, config, and TPROXY scripts
sudo install -o root -g root -m 0755 target/release/rog /usr/local/bin/rog
install -o "$ADMIN_USER" -g rog -m 0664 config.toml /etc/rog/config.toml
install -o "$ADMIN_USER" -g rog -m 0775 scripts/tproxy-up.sh /etc/rog/tproxy-up.sh
install -o "$ADMIN_USER" -g rog -m 0775 scripts/tproxy-down.sh /etc/rog/tproxy-down.sh
```

#### 3.3 Bypass TPROXY for Other Local Daemons (e.g. `cloudflared`)
To exempt another local daemon on the gateway from TPROXY interception, assign it `Group=rog` via a systemd drop-in so it matches `--gid-owner rog`:
```bash
sudo mkdir -p /etc/systemd/system/cloudflared.service.d
sudo tee /etc/systemd/system/cloudflared.service.d/rog-bypass.conf >/dev/null <<'EOF'
[Service]
Group=rog
EOF
sudo systemctl daemon-reload && sudo systemctl restart cloudflared
```

#### 3.4 Create `/etc/systemd/system/rog.service`

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
Environment=RUST_LOG=info
Environment=PROXY_OBSERVE_LISTEN=0.0.0.0:19090
Environment=PROXY_OBSERVE_TOKEN=token
AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE CAP_BPF CAP_PERFMON
CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE CAP_BPF CAP_PERFMON
NoNewPrivileges=true
ExecStartPre=+/etc/rog/tproxy-up.sh
ExecStart=/usr/local/bin/rog
ExecStopPost=+/etc/rog/tproxy-down.sh
Restart=on-failure
RestartSec=3s
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
```

---

### 4. Customizing `tproxy-up.sh` / `tproxy-down.sh` & Line-by-Line `iptables` Explanation

#### 4.1 Customizable Parameters & Environment Variables
`scripts/tproxy-up.sh` and `scripts/tproxy-down.sh` auto-detect network interfaces and subnets by default, and can be customized via positional arguments `[TPROXY_PORT] [FWMARK] [TABLE_ID] [MODE] [FAMILY]` or environment variables:

| Variable | Default | Description |
| :--- | :--- | :--- |
| `ROG_PORT` | `12345` | `tproxy` listener port in `rog` |
| `ROG_FWMARK` | `0x77` | Policy routing firewall mark for TPROXY sockets |
| `ROG_TABLE` | `2809` | Policy routing table ID and rule priority (`pref 2809 lookup 2809`) |
| `ROG_MODE` | `tcp+udp` | Intercepted protocols: `tcp+udp`, `tcp`, or `udp` |
| `ROG_FAMILY` | `dual` | Address family: `dual` (IPv4+IPv6), `v4`, or `v6` |
| `ROG_IFACE` | *(auto)* | Primary LAN interface (auto-detected from default route, e.g. `eth0`) |
| `ROG_IPV4` | *(auto)* | Gateway's local IPv4 address on `ROG_IFACE` (e.g. `192.168.0.71`) |
| `ROG_LAN_CIDR` | *(auto)* | Local LAN IPv4 CIDR on `ROG_IFACE` (e.g. `192.168.0.0/23`) |
| `ROG_DNS_PORT` | `53` | Local UDP DNS port to redirect port 53 queries to (`0` to disable) |
| `ROG_USER` / `ROG_GROUP` | `rog` / `rog` | Service user and group exempted in `OUTPUT` chains |
| `ROG_BYPASS_IPV4` | *(empty)* | Extra space-separated IPv4 CIDRs to bypass TPROXY |

#### 4.2 Startup Commands Line-by-Line (`tproxy-up.sh`)

Substituting concrete example values (`eth0`, `192.168.0.71`, `192.168.0.0/23`, port `12345`, mark `0x77`, table `2809`, `rog` UID=`999`/GID=`990`):

```bash
# 1. Load kernel modules and configure policy routing table 2809 (delivers 0x77-marked packets to lo)
modprobe -a nf_tproxy_ipv4 nf_tproxy_ipv6 xt_TPROXY xt_socket xt_owner cls_bpf sch_clsact 2>/dev/null || true
ip -4 rule add pref 2809 fwmark 0x77/0xff lookup 2809
ip -4 route replace local 0.0.0.0/0 dev lo table 2809

# 2. Create/flush dedicated custom chains in mangle and nat tables
iptables -w -t mangle -N ROG_TP      # Forwarded LAN traffic (PREROUTING)
iptables -w -t mangle -N ROG_OUT     # Gateway's own local traffic (OUTPUT)
iptables -w -t nat -N ROG_NAT        # MASQUERADE for LAN direct L3 forwarding
iptables -w -t nat -N ROG_DNS_PRE    # Redirect LAN UDP 53 DNS to rog
iptables -w -t nat -N ROG_DNS_OUT    # Redirect local host UDP 53 DNS to rog

# 3. Bypass local host IPs (e.g. SSH :22), UDP :53, and private/LAN subnets in both ROG_TP and ROG_OUT
iptables -w -t mangle -A ROG_TP/ROG_OUT -m addrtype --dst-type LOCAL -j RETURN
iptables -w -t mangle -A ROG_TP/ROG_OUT -p udp --dport 53 -j RETURN
# (Followed by -d 0.0.0.0/8, 10.0.0.0/8, 127.0.0.0/8, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16, 198.18.0.0/15, 224.0.0.0/4, 240.0.0.0/4 -j RETURN)

# 4. ROG_OUT: exempt non-local forwarded packets and rog's own UID/GID, then mark local TCP/UDP with 0x77 to reroute via lo
iptables -w -t mangle -I ROG_OUT 1 -m addrtype ! --src-type LOCAL -j RETURN
iptables -w -t mangle -A ROG_OUT -m owner --uid-owner 999 -j RETURN
iptables -w -t mangle -A ROG_OUT -m owner --gid-owner 990 -j RETURN
iptables -w -t mangle -A ROG_OUT -p tcp -j MARK --set-xmark 0x77/0xff
iptables -w -t mangle -A ROG_OUT -p udp -j MARK --set-xmark 0x77/0xff

# 5. ROG_TP: fast-path established transparent TCP sockets, bypass eBPF 0x78-marked Direct packets, and TPROXY the rest to :12345
iptables -w -t mangle -A ROG_TP -p tcp -m socket --transparent --nowildcard -j MARK --set-xmark 0x77/0xff
iptables -w -t mangle -A ROG_TP -p tcp -m socket --transparent --nowildcard -j ACCEPT
iptables -w -t mangle -A ROG_TP -m mark --mark 0x78/0xff -j RETURN
iptables -w -t mangle -A ROG_TP -p tcp -j TPROXY --on-port 12345 --tproxy-mark 0x77/0xff
iptables -w -t mangle -A ROG_TP -p udp -j TPROXY --on-port 12345 --tproxy-mark 0x77/0xff

# 6. Hook ROG_TP and ROG_OUT into mangle PREROUTING and OUTPUT
iptables -w -t mangle -A PREROUTING -i eth0 -s 192.168.0.0/23 -j ROG_TP
iptables -w -t mangle -A PREROUTING -i lo -m mark --mark 0x77/0xff -j ROG_TP
iptables -w -t mangle -A OUTPUT -j ROG_OUT

# 7. Hook DNS redirection (:53) and LAN MASQUERADE into nat PREROUTING, OUTPUT, and POSTROUTING
iptables -w -t nat -A ROG_DNS_PRE -p udp --dport 53 ! -d 192.168.0.71/32 -j DNAT --to-destination 192.168.0.71:53
iptables -w -t nat -A ROG_DNS_OUT -m addrtype ! --src-type LOCAL -j RETURN
iptables -w -t nat -A ROG_DNS_OUT -m owner --uid-owner 999 -j RETURN
iptables -w -t nat -A ROG_DNS_OUT -m owner --gid-owner 990 -j RETURN
iptables -w -t nat -A ROG_DNS_OUT -p udp --dport 53 ! -d 192.168.0.71/32 -j DNAT --to-destination 192.168.0.71:53
iptables -w -t nat -A ROG_NAT -s 192.168.0.0/23 -o eth0 -j MASQUERADE
iptables -w -t nat -A PREROUTING -i eth0 -s 192.168.0.0/23 -j ROG_DNS_PRE
iptables -w -t nat -A OUTPUT -j ROG_DNS_OUT
iptables -w -t nat -A POSTROUTING -j ROG_NAT
```

#### 4.3 Teardown Commands Line-by-Line (`tproxy-down.sh`)

On stop, `tproxy-down.sh` removes only `rog`'s own jump rules, custom chains, and policy routing table `2809` without flushing unrelated system rules:

```bash
# 1. Remove jump rules from PREROUTING, OUTPUT, and POSTROUTING
while iptables -w -t mangle -D PREROUTING -i eth0 -s 192.168.0.0/23 -j ROG_TP 2>/dev/null; do :; done
while iptables -w -t mangle -D PREROUTING -i lo -m mark --mark 0x77/0xff -j ROG_TP 2>/dev/null; do :; done
while iptables -w -t mangle -D OUTPUT -j ROG_OUT 2>/dev/null; do :; done
while iptables -w -t nat -D PREROUTING -i eth0 -s 192.168.0.0/23 -j ROG_DNS_PRE 2>/dev/null; do :; done
while iptables -w -t nat -D OUTPUT -j ROG_DNS_OUT 2>/dev/null; do :; done
while iptables -w -t nat -D POSTROUTING -j ROG_NAT 2>/dev/null; do :; done

# 2. Flush and delete custom chains
for chain in ROG_TP ROG_OUT; do iptables -w -t mangle -F "$chain"; iptables -w -t mangle -X "$chain"; done
for chain in ROG_NAT ROG_DNS_PRE ROG_DNS_OUT; do iptables -w -t nat -F "$chain"; iptables -w -t nat -X "$chain"; done

# 3. Remove policy routing rule and flush table 2809
while ip -4 rule del pref 2809 fwmark 0x77/0xff lookup 2809 2>/dev/null; do :; done
ip -4 route flush table 2809 2>/dev/null || true
```

### 5. LAN Client Configuration

- **IPv4**: Point the client's **Default Gateway (Router)** and **DNS Server** to the side-gateway's IPv4 address.
- **IPv6**:
  - Because `suppress_ipv6_ra = true` is enabled by default, active side-gateway clients automatically ignore the main router's IPv6 RA / RDNSS.
  - To route client IPv6 traffic through the side-gateway, point the client's **IPv6 Router** to the side-gateway's link-local address (`fe80::...` under `scope link` on the LAN interface), or set `disable_aaaa = true` under `[dns]` for IPv4-only client routing.

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
