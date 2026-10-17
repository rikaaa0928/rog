# rog

一个灵活且强大的网络代理。

## 特性

- **多协议支持:** 支持多种入站/出站协议，包括 TCP、HTTP、SOCKS5、HTSS5、PB-TCP、gRPC、反向 gRPC (`rev_grpc`) 以及 Linux `TPROXY`（支持 IPv4/IPv6 双栈透明代理）。
- **可配置路由:** 支持基于 CIDR、正则、域名后缀、域名关键字及远端动态规则集的灵活分流。
- **异步架构:** 使用 Tokio 构建，实现高性能和非阻塞操作。
- **集中式配置:** 通过 TOML 文件管理配置。
- **内存管理:** 可配置全局缓冲区大小，支持基于百分比的限制，有效控制内存使用。
- **独立 UDP DNS 与域名映射学习:** 内置按路由分流的双栈 UDP DNS 服务，自动学习真实 IP（A/AAAA）→ 域名映射供 TPROXY 透明分流与跨栈代理转发使用。

## 入门指南

### 前提条件

- Rust (最新稳定版本)
- Cargo

### 构建

```bash
cargo build --release
```

### 运行

默认情况下，rog 会在 `/etc/rog/config.toml` 查找配置文件。你可以使用 `ROG_CONFIG` 环境变量指定不同的路径：

```bash
export ROG_CONFIG=/path/to/your/config.toml
./target/release/rog
```

### 配置

配置通过 TOML 文件进行管理。

#### 全局配置

- `server_id`: (可选) 此代理实例的唯一标识符。
- `buffer_size`: (可选) 全局 TCP 缓冲区限制。支持字节（例如 "64MB"）、系统内存百分比（例如 "50%"）或 "off" 以禁用。

以下是一个示例结构：

```toml
# 全局设置
server_id = "my-proxy-01"
buffer_size = "50%" # 使用 50% 的可用内存作为 TCP 缓冲区

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

- `endpoint`: 监听的地址和端口（如 `"0.0.0.0:1080"` 或双栈 `"[::]:12345"`）。
- `name`: 监听器的唯一名称。
- `proto`: 使用的协议（如 `"tcp"`, `"http"`, `"socks5"`, `"htss5"`, `"pb_tcp"`, `"grpc"`, `"rev_grpc"`, `"tproxy"`）。
- `router`: 此监听器使用的路由器的名称。
- `options`: 可选协议参数。
  - `tproxy` 支持 `mode`（`"tcp+udp"` 默认、`"tcp"`、`"udp"`）、`udp_idle_timeout_secs`、`udp_max_senders`。
  - `grpc`/`rev_grpc` 支持 `keep_alive`、`keep_alive_interval_secs`、`keep_alive_timeout_secs`、`keep_alive_while_idle`、`connect_timeout_secs`、`tcp_nodelay`、`tcp_keepalive_secs`、`tcp_keepalive_interval_secs`、`tcp_keepalive_retries`、`initial_stream_window_size`、`initial_connection_window_size`、`stream_channel_size`、`channel_buffer_size`、`max_decoding_message_size`、`max_encoding_message_size`、`max_concurrent_streams`、`max_frame_size`、`concurrency_limit`、`concurrency_limit_per_connection` 等。

#### `router`

- `name`: 路由器的唯一名称。
- `default`: 要使用的默认路由（connector 名称）。
- `route_rules`: 路由规则列表。

#### `router.route_rules`

- `name`: 引用的 `data` 数据源名称。
- `select`: 命中后选择的 `connector` 名称。
- `exclude`: 要排除的数据源名称列表。
- `domain_to_ip`: (可选) 是否将域名解析为 IP 地址后再匹配 CIDR 规则。
- `dns`: (可选) `domain_to_ip` 解析时专用的 UDP DNS 服务器 (例如 `"223.5.5.5:53"`)。

#### `data`

- `name`: 数据源的唯一名称。
- `url`: 用于加载数据的可选 URL（支持 `http://`、`https://`、`file://`，并支持在 URL 末尾附加 `#interval=3600` 指定刷新间隔秒数）。
- `format`: 数据格式。支持 `"cidr"`, `"regex"`, `"lan"`, `"domain-suffix"` (或 `"suffix"`, `"domain_suffix"`), `"domain-keyword"` (或 `"keyword"`, `"domain_keyword"`)。
- `interval`: (可选) 定时刷新间隔（秒），仅在配置了 `url` 时生效，默认 3600 秒（若 `url` 中带有 `#interval=<秒>` 则优先使用 URL 中的值）。加载完全非阻塞，失败自动保持旧数据重试。
- `data`: 内联数据（多行字符串）。

#### `connector`

- `endpoint`: 连接器的端点。
- `name`: 连接器的唯一名称。
- `user`: 用于身份验证的可选用户名。
- `pw`: 用于身份验证的可选密码。
- `proto`: 连接器的协议（如 `"tcp"`, `"socks5"`, `"http"`, `"htss5"`, `"pb_tcp"`, `"grpc"`, `"rev_grpc"`, `"block"`）。
- `dns`: (可选) 该出口关联的上游 UDP DNS 列表（如 `["114.114.114.114:53", "223.5.5.5:53"]`）。未配置时默认回退使用 `[dns]` 中的 `default_servers`。
- `options`: 可选协议参数。
  - `bootstrap_dns`: (可选) 代理节点域名自身的启动解析 DNS 列表。未配置时优先回退使用该 connector 的 `dns`，若仍未配置则回退使用 `[dns]` 中的 `default_servers`。
  - `grpc` 支持与 listener 相同的 gRPC transport 参数；常用推荐值见 `full_config_example.toml`。

## 用法

以下是如何配置 rog 以充当 SOCKS5 代理的示例：

**1. 创建一个 `config.toml` 文件:**

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
endpoint = "127.0.0.1:9050" # 替换为你的上游 SOCKS5 代理
```

此配置在 `0.0.0.0:1080` 上设置了一个 SOCKS5 监听器。所有流量将使用 `default` 路由器进行路由。`default` 路由器有一个规则，将发送到 `*.example.com` 的流量转发到 `proxy_connector`。`proxy_connector` 被配置为一个连接到 `127.0.0.1:9050` 的 SOCKS5 客户端。

**2. 运行代理:**

```bash
./target/release/rog
```

现在，你可以配置你的应用程序以使用 `localhost:1080` 作为 SOCKS5 代理。对以 `example.com` 结尾的域名的任何请求都将转发到 `127.0.0.1:9050` 的上游代理。

**注意:** 将 `127.0.0.1:9050` 替换为你的上游 SOCKS5 代理服务器的实际地址。

## 贡献

欢迎贡献! 请随时提交 pull request 或打开 issue。

## 许可证

本项目根据 [LICENSE NAME] 许可证获得许可。

## TPROXY 透明代理与旁网关部署指南（Linux 双栈）

`rog` 支持在 Linux 上通过 **`TPROXY` 入站 + 独立 `UDP DNS` 服务** 构建高性能透明代理或局域网旁路由（旁网关），同时支持 **IPv4 与 IPv6 双栈**、**TCP 与 UDP 全流量透明拦截**。

### 1. 工作原理

相比传统的 TUN + Fake-IP 方案，`rog` 的 `TPROXY + 真实 DNS` 模式无需创建虚拟网卡，也不会向客户端返回虚假的 `198.18.x.x` IP：
1. **DNS 查询与映射学习**：客户端将 DNS 指向旁网关（或被 `nat` 表 53 端口规则透明重定向到 `rog` 的 `[::]:53`）。`rog` 根据 `router` 中的域名规则分流查询上游 DNS，将**真实公网 IP（A / AAAA）**返回给客户端，同时在内存 `IpDomainTable` 中记录 `真实目标 IP -> 域名` 的映射关系。
2. **TCP / UDP 透明拦截与域名反查**：客户端向该真实公网 IP 发起 TCP 或 UDP（如 QUIC / NTP）连接时，Linux 内核 `TPROXY` 在不修改数据包源/目地址的情况下将流量转交 `rog`（`[::]:12345`）。`rog` 通过目标 IP 反查 `IpDomainTable` 还原出原始域名，从而精确执行基于域名的分流规则。
3. **跨栈代理转发（IPv6 客户端 $\rightarrow$ IPv4 远端节点）**：当客户端通过 IPv6 访问某域名，而命中的远端代理服务器仅具备 IPv4 出站能力时，`rog` 会自动将连接目标从 IPv6 字面量还原为学习到的域名发往远端代理节点，由远端节点自行解析 IPv4 连接目标站点。

### 2. 完整旁网关配置示例 (`/etc/rog/config.toml`)

```toml
[dns]
endpoint = "[::]:53"
router = "main"
default_servers = ["1.1.1.1:53", "8.8.8.8:53"]
disable_aaaa = false # 如不希望局域网使用 IPv6，可设为 true 屏蔽 AAAA 记录
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
proto = "grpc"
endpoint = "https://proxy.example.com:443"
pw = "your_password"
# 未配置 dns 和 bootstrap_dns 时，自动回退使用 [dns].default_servers
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
dns = ["114.114.114.114:53", "223.5.5.5:53"] # 国内直连出口指定国内 DNS

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

### 3. 部署步骤与 Systemd 服务配置

由于 `scripts/tproxy-up.sh` 同时拦截了**局域网转发流量 (`PREROUTING`)** 和 **旁网关本机自身发出的流量 (`OUTPUT`)**，为了防止 `rog` 自己发出的代理连接再次被 `OUTPUT` 链拦截造成死循环，**必须使用独立的系统用户/组（默认 `rog:rog`）运行 `rog`**，并由 `systemd` 的 `ExecStartPre=+` 与 `ExecStopPost=+` 统一装载和卸载 TPROXY 内核规则：
- 仓库 `scripts/tproxy-up.sh` 与 `scripts/tproxy-down.sh` 默认**全机器通用自动探测**：自动从默认路由探测主网卡（`ROG_IFACE`）、本机 IPv4（`ROG_IPV4`）、局域网网段（`ROG_LAN_CIDR`）以及网卡 IPv6 RA 前缀，并自动设置 `sysctl` 转发参数、策略路由、双栈 `mangle` TPROXY 链、UDP `:53` 透明重定向与 `MASQUERADE`。
- 如需在特殊网络拓扑下自定义，可在 `rog.service` 中通过脚本位置参数 `[TPROXY_PORT] [FWMARK] [TABLE_ID] [MODE] [FAMILY]` 或 `Environment=` 环境变量覆盖：
  - `ROG_IFACE`：局域网网卡名（默认自动取默认路由网卡，如 `eth0`）
  - `ROG_IPV4`：旁网关本机 IPv4 地址（默认自动取 `ROG_IFACE` 全局 IPv4）
  - `ROG_LAN_CIDR`：局域网网段 CIDR（默认自动取 `ROG_IFACE` 直连路由网段）
  - `ROG_DNS_PORT`：透明劫持 UDP 53 到的本地 DNS 端口（默认 `53`，设为 `0` 关闭 DNS 端口劫持）
  - `ROG_USER` / `ROG_GROUP`：`OUTPUT` 链直连豁免的服务运行用户与组（默认 `rog`，防止回环死循环）
  - `ROG_BYPASS_IPV4`：额外需要直连绕过 TPROXY 的 IPv4 CIDR 列表（空格分隔，可选）

```bash
# 1. 创建专用系统用户和配置目录（允许当前登录用户直接编辑配置文件）
ADMIN_USER="$(id -un)"
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin rog || true
sudo install -d -o "$ADMIN_USER" -g rog -m 2775 /etc/rog

# 2. 拷贝二进制、配置与 TPROXY 脚本
sudo install -o root -g root -m 0755 target/release/rog /usr/local/bin/rog
install -m 0775 scripts/tproxy-up.sh /etc/rog/tproxy-up.sh
install -m 0775 scripts/tproxy-down.sh /etc/rog/tproxy-down.sh
```

创建 `/etc/systemd/system/rog.service`：

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

*(提示：如果旁网关本机上还运行了 `cloudflared` 等需要直连绕过 TPROXY 的服务，只需在对应服务的 systemd drop-in 中加上 `Group=rog`，即可自动命中 `--gid-owner` 规则直连放行。)*

### 4. 局域网客户端接入方式

- **IPv4 设置**：将客户端（或主路由 DHCP 下发）的 **默认网关（Router）** 和 **DNS 服务器** 均指向旁网关的 IPv4 地址（例如 `192.168.0.71`）。
- **IPv6 设置**：
  - **方式 A（开启完整 IPv6 代理）**：将客户端的 **IPv6 路由器（Router）** 指向旁网关网卡的 `fe80::...` 链路本地地址（`ip -6 addr show` 中 `scope link` 的地址，永久不变）。
  - **方式 B（仅配 IPv4 网关、不配 IPv6 网关）**：若局域网主路由开启了 IPv6 RA 广播，而你不希望在客户端手动填写 IPv6 网关，只需在 `/etc/rog/config.toml` 的 `[dns]` 中设置 `disable_aaaa = true`，让客户端所有域名请求统一通过 IPv4 经过旁网关分流。

## 独立 UDP DNS 服务

在配置中增加 `[dns]` 即可启动独立 UDP DNS 服务（支持监听双栈 `"[::]:53"`），客户端直接把 DNS 指向 rog 所在机器。DNS 复用 `dns.router` 引用的现有业务规则，跳过纯 IP 规则并禁止 `domain_to_ip`；每条查询按域名匹配规则选择上游 DNS：
- 当命中的 connector 配置了专属 `dns`（且不同于 `dns.default_servers`，如国内直连 `Direct` 配置 `["114.114.114.114:53", "223.5.5.5:53"]`）时，通过该 connector 查询其专属 DNS；
- 当命中的代理 connector 未配置专属 `dns`（或与 `dns.default_servers` 相同）时，默认复用路由器 `default` 出口查询 `dns.default_servers`。
- 支持配置 `disable_aaaa = true` 屏蔽 `AAAA` 记录（直接返回空 `NOERROR` 响应），默认为 `false`。

参考 `dns_config_example.toml` 和 `full_config_example.toml`。本机测试示例监听 `127.0.0.1:5353`，可使用 `dig @127.0.0.1 -p 5353 example.com` 验证；生产环境通常监听 `"[::]:53"`。TPROXY 和 DNS 在同一进程使用同一个 `router` 时，会自动共享真實 IP（IPv4/IPv6）→ 域名映射表：
- TPROXY 收到客户端连接后，自动根据目标 IP 反查已学习的域名进行域名规则分流；
- 当客户端发起 IPv6 连接且命中的代理出口为非直连节点（如仅支持 IPv4 出站的远端代理服务器）时，自动将目标 IPv6 地址还原为学习到的域名发往远端代理节点解析连接。

代理节点域名自身的启动解析（`bootstrap_dns`）按以下优先级自动回退，无需重复配置：
`connector.options.bootstrap_dns` $\rightarrow$ `connector.dns` $\rightarrow$ `dns.default_servers`。节点域名通过这些服务器直连解析，绕过系统 DNS 和业务 DNS 路由，gRPC 保留域名 TLS/SNI。

## DNS 映射观测

设置 `PROXY_OBSERVE_LISTEN=127.0.0.1:18080` 开启可观测 API，可用
`PROXY_OBSERVE_TOKEN` 设置访问令牌。`GET /api/v1/observe/ip-domains` 返回当前
独立 UDP DNS 学习到的有效 IP → 域名关系，使用与其他观测接口相同的鉴权。

```bash
curl -H 'Authorization: Bearer <token>' \
  'http://127.0.0.1:18080/api/v1/observe/ip-domains?search=example&limit=200&offset=0'
```

每条记录包含 `router`、`client_ip`、`target_ip`、`domain`、`ttl_remaining_ms`
（剩余 TTL）和 `last_observed_ago_ms`（距离最近一次观测的时间）。同一个 IP
对应多个域名时逐条展示；不同客户端及 router 的映射相互隔离。过期记录不返回，
查询不会刷新 TTL 或改变路由状态。此接口展示映射缓存，不表示当前连接使用了哪条路由。

支持 `router` 精确筛选、`client_ip` / `target_ip` 地址筛选、`domain` 子串筛选，
以及 `search` 在 router、两个 IP 和域名之间搜索。`offset` 默认 0，`limit` 默认
200、范围 1–1000，按 router、客户端、目标 IP、域名排序后分页。
`total` 是筛选后的记录总数；`key_count`、`record_count` 是整个有效缓存的键数及
域名记录数，不受筛选和分页影响。`capacity` 是最大键数，键为
`(router, client_ip, target_ip)`；同时返回 `max_domains_per_ip` 和 `max_ttl_secs`。
未配置 `[dns]` 时返回 `enabled: false` 和空记录。

相邻 `proxy-observe-ui` 客户端的 **IP Domains** 页支持搜索、分页、列排序和自动刷新。
连接没有该接口的旧服务时显示不支持提示，其他观测页面仍然可用。
