# rog

一个灵活且强大的网络代理。

## 特性

- **多协议支持:** 支持多种入站/出站协议，包括 TCP、HTTP、SOCKS5、HTSS5、PB-TCP、gRPC、反向 gRPC (`rev_grpc`) 以及 Linux `TPROXY`（支持 IPv4/IPv6 双栈透明代理）。
- **可配置路由:** 支持基于 CIDR、正则、域名后缀、域名关键字及远端动态规则集的灵活分流。
- **异步架构:** 使用 Tokio 构建，实现高性能和非阻塞操作。
- **集中式配置:** 通过 TOML 文件管理配置。
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

以下是一个示例结构：

```toml
# 全局设置
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

相比传统的 TUN + Fake-IP 方案，`rog` 的 `TPROXY + 真实 DNS + eBPF 直连快路` 模式无需创建虚拟网卡，也不会向客户端返回虚假的 `198.18.x.x` IP：
1. **DNS 查询与映射学习**：客户端将 DNS 指向旁网关（或被 `nat` 表 53 端口规则透明重定向到 `rog` 的 `[::]:53`）。`rog` 根据 `router` 中的域名规则分流查询上游 DNS，将**真实公网 IP（A / AAAA）**返回给客户端，同时在内存 `IpDomainTable` 中记录 `真实目标 IP -> 域名` 的映射关系。
2. **eBPF TC Ingress 直连内核快路旁路（`ebpf_direct_offload = true`，默认开启）**：
   - `rog` 启动时使用纯 Rust 原生 `SYS_bpf` 与 `AF_NETLINK`（零外部 `clang`/`llvm`/`tc` 依赖）在主网卡 `tc ingress` 挂载 eBPF 程序 `rog_direct_tc`，并在内核维护 4 张双栈 BPF Map：`v4_lpm` / `v6_lpm`（`BPF_MAP_TYPE_LPM_TRIE`，自动同步静态与远程 `Direct` CIDR 网段）与 `v4_lru` / `v6_lru`（`BPF_MAP_TYPE_LRU_HASH`，在 DNS 解析出 `Direct` 域名时实时写入返回的 A/AAAA IP 及内核单调时钟过期时间；若某共享 CDN IP 后续被代理域名解析则自动从内核表撤回）。
   - 客户端发起连接时，命中 `Direct` 的数据包在网卡 `tc ingress` 层即被 eBPF 打上 `0x78` 标记，在 `iptables`/`ip6tables` 的 `ROG_TP`/`ROG6_TP` 链中直接 `RETURN` 走 Linux 内核原生三层转发（零用户态唤醒、零套接字创建、零内存拷贝）。
3. **TCP / UDP 透明拦截与域名反查**：需要走代理的流量由 Linux 内核 `TPROXY` 在不修改数据包源/目地址的情况下转交 `rog`（`[::]:12345`）。`rog` 通过目标 IP 反查 `IpDomainTable` 还原出原始域名，精确执行分流规则。
4. **跨栈代理转发（IPv6 客户端 $\rightarrow$ IPv4 远端节点）**：当客户端通过 IPv6 访问某域名，而命中的远端代理服务器仅具备 IPv4 出站能力时，`rog` 会自动将连接目标从 IPv6 字面量还原为学习到的域名发往远端代理节点，由远端节点自行解析 IPv4 连接目标站点。
5. **自动压制主路由 IPv6 RA / RDNSS（`suppress_ipv6_ra = true`，默认开启）**：
   - 在家用主路由开启 IPv6 RA 的局域网中，Android / Chromecast / Google TV 等设备即使手动配置了静态 IPv4 网关与 DNS，仍会强制采纳主路由通过 IPv6 RA 组播下发的 `RDNSS`（IPv6 DNS）和 IPv6 默认网关，导致 DNS 污染和流量旁路。
   - `rog` 会根据经过 `dns` 或 `tproxy` 的实时流量自动识别活跃旁路由客户端，从 `/proc/net/arp` 解析其二层 MAC 地址，并向该客户端 MAC 二层单播发送 `lifetime = 0` 的抵消 RA 帧（自动清除主路由注入的 IPv6 网关与 RDNSS，完全不影响局域网其他设备）。当客户端切换回主路由、连续无流量超过 `suppress_ipv6_ra_ttl_secs`（默认 `300` 秒）后自动移出压制列表。

### 2. 完整旁网关配置示例 (`/etc/rog/config.toml`)

```toml
[dns]
endpoint = "[::]:53"
router = "main"
default_servers = ["1.1.1.1:53", "8.8.8.8:53"]
bootstrap_servers = ["114.114.114.114:53", "223.5.5.5:53"]
disable_aaaa = false # 如不希望局域网客户端解析 IPv6，可设为 true 屏蔽 AAAA 记录
suppress_ipv6_ra = true # 默认开启：自动向活跃旁路由客户端单播清零主路由 IPv6 RA/RDNSS
suppress_ipv6_ra_ttl_secs = 300 # 活跃客户端无流量过期移出时间（秒，默认 300）
ebpf_direct_offload = true # 默认开启：eBPF TC 内核态 Direct 直连快路旁路
ebpf_direct_connectors = ["Direct"] # 可选：指定由 eBPF 接管的直连 connector 名称；未配置时默认匹配所有 proto = "tcp" 的 connector
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
# 未配置 dns 和 bootstrap_dns 时，自动回退使用 [dns].default_servers / bootstrap_servers
[connector.options]
connect_timeout_secs = 5
keep_alive = true
keep_alive_interval_secs = 25
max_idle_timeout_secs = 90
connections = 2
fallback_h2 = true # 如需跳过 QUIC 直接使用 HTTP/2 over TLS 连接，可将 endpoint 写为 h2://...
tcp_nodelay = true
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

### 3. 建立专用系统用户/用户组与 Systemd 服务配置

#### 3.1 为什么必须建立独立的 `rog` 用户和用户组？
由于 `scripts/tproxy-up.sh` 同时拦截了**局域网客户端转发流量 (`PREROUTING`)** 和 **旁网关本机自身发出的流量 (`OUTPUT`)**：
- 如果以 `root` 或普通用户直接运行 `rog`，那么 `rog` 向远端代理节点（或上游 DNS）发起的出站连接也会被本机 `OUTPUT` 链拦截并塞回 `rog` 自己，造成**无限回环死循环**。
- 因此，**必须创建专用的系统用户与用户组 `rog:rog` 来运行 `rog`**。`tproxy-up.sh` 会在 `OUTPUT` 链中通过 `-m owner --uid-owner rog` 和 `-m owner --gid-owner rog` 将 `rog` 用户及 `rog` 用户组的流量直接放行直连公网。

#### 3.2 创建 `rog` 用户/组与配置目录权限
```bash
# 1. 创建无登录权限的专用系统用户 rog 与同名用户组 rog
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin rog || true

# 2. 创建 /etc/rog 配置目录：属主设为当前管理员用户，属组设为 rog，
#    并开启 SGID 位 (2775)，使新建文件自动继承 rog 属组，方便日常免 sudo 编辑配置
ADMIN_USER="$(id -un)"
sudo install -d -o "$ADMIN_USER" -g rog -m 2775 /etc/rog

# 3. 安装二进制文件、配置文件与启停脚本
sudo install -o root -g root -m 0755 target/release/rog /usr/local/bin/rog
install -o "$ADMIN_USER" -g rog -m 0664 config.toml /etc/rog/config.toml
install -o "$ADMIN_USER" -g rog -m 0775 scripts/tproxy-up.sh /etc/rog/tproxy-up.sh
install -o "$ADMIN_USER" -g rog -m 0775 scripts/tproxy-down.sh /etc/rog/tproxy-down.sh
```

#### 3.3 让本机其他直连服务（如 `cloudflared` / `tailscaled`）绕过 TPROXY
如果旁网关本机还运行了内网穿透或隧道服务（如 `cloudflared`），不希望其流量走 TPROXY，只需将其附加到 `rog` 用户组（命中 `--gid-owner rog` 豁免规则）：
```bash
sudo mkdir -p /etc/systemd/system/cloudflared.service.d
sudo tee /etc/systemd/system/cloudflared.service.d/rog-bypass.conf >/dev/null <<'EOF'
[Service]
Group=rog
EOF
sudo systemctl daemon-reload && sudo systemctl restart cloudflared
```

#### 3.4 配置 `/etc/systemd/system/rog.service`
- `ExecStartPre=+/etc/rog/tproxy-up.sh` 与 `ExecStopPost=+/etc/rog/tproxy-down.sh` 前面的 `+` 号表示以 `root` 权限执行防火墙与路由表挂载/卸载；
- `ExecStart=/usr/local/bin/rog` 则降权以 `User=rog` / `Group=rog` 运行，并通过 `AmbientCapabilities` 仅授予网络管理与 eBPF 所需的最小内核能力（`CAP_NET_ADMIN`、`CAP_NET_RAW`、`CAP_NET_BIND_SERVICE`、`CAP_BPF`、`CAP_PERFMON`）：

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

### 4. 如何自定义配置 `iptables` 脚本及逐行原理解析

`scripts/tproxy-up.sh` 与 `scripts/tproxy-down.sh` 默认**全机器通用自动探测**，无需硬编码网卡名或 IP。

#### 4.1 支持自定义的环境变量与位置参数
可通过脚本位置参数 `./tproxy-up.sh [TPROXY_PORT] [FWMARK] [TABLE_ID] [MODE] [FAMILY]` 或在 `rog.service` 中添加 `Environment="KEY=VALUE"` 进行自定义：

| 变量名 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `ROG_PORT` | `12345` | `rog` 中 `tproxy` 监听器绑定的端口号 |
| `ROG_FWMARK` | `0x77` | 策略路由标记（匹配该标记的数据包将投递给本机 TPROXY 套接字） |
| `ROG_TABLE` | `2809` | 策略路由表编号与规则优先级（`pref 2809 lookup 2809`） |
| `ROG_MODE` | `tcp+udp` | 拦截协议类型：可选 `tcp+udp`、`tcp`、`udp` |
| `ROG_FAMILY` | `dual` | 协议栈类型：可选 `dual`（IPv4+IPv6 双栈）、`v4`、`v6` |
| `ROG_IFACE` | *(自动探测)* | 主局域网网卡名（默认从 `ip -4 route show default` 自动提取，如 `eth0`） |
| `ROG_IPV4` | *(自动探测)* | 旁网关本机 IPv4 地址（默认自动取 `ROG_IFACE` 上的全局 IPv4，如 `192.168.0.71`） |
| `ROG_LAN_CIDR` | *(自动探测)* | 局域网网段 CIDR（默认自动取 `ROG_IFACE` 直连链路网段，如 `192.168.0.0/23`） |
| `ROG_DNS_PORT` | `53` | 透明劫持 UDP 53 流量到的本机端口（设为 `0` 则禁用 `nat` 表 DNS 劫持） |
| `ROG_USER` / `ROG_GROUP` | `rog` / `rog` | 在 `OUTPUT` 链中直连放行的用户与用户组 |
| `ROG_BYPASS_IPV4` | *(空)* | 额外静态绕过 TPROXY 的 IPv4 CIDR 列表（空格分隔，如 `"1.2.3.4/32 5.6.7.0/24"`） |

---

#### 4.2 启动时执行的命令逐行详解（`tproxy-up.sh`）

以下将脚本中的变量代入典型旁网关环境（网卡 `eth0`、本机 IP `192.168.0.71`、局域网 `192.168.0.0/23`、TPROXY 端口 `12345`、标记 `0x77`、路由表 `2809`、`rog` UID=`999`/GID=`990`）进行逐行解释：

##### （1）加载内核模块与策略路由（将打上 `0x77` 的包送入本机 `lo`）
```bash
# 加载 TPROXY、透明 socket 匹配、进程属主匹配以及 TC eBPF 所需的内核模块
modprobe -a nf_tproxy_ipv4 nf_tproxy_ipv6 xt_TPROXY xt_socket xt_owner cls_bpf sch_clsact 2>/dev/null || true

# 添加策略路由规则（优先级 2809，高于系统默认 main 表的 32766）：凡带有 0x77 标记的包查询 2809 号路由表
ip -4 rule add pref 2809 fwmark 0x77/0xff lookup 2809

# 在 2809 号路由表中，将所有目标 IP (0.0.0.0/0) 视为本机 local 地址，经 lo 口上送给监听在 :12345 的 rog 进程
ip -4 route replace local 0.0.0.0/0 dev lo table 2809
```
*(IPv6 对称执行 `ip -6 rule add pref 2809 fwmark 0x77/0xff lookup 2809` 与 `ip -6 route replace local ::/0 dev lo table 2809`)*

##### （2）创建或清空专属自定义链（与系统其他防火墙规则完全隔离）
```bash
# mangle 表：负责流量分类与 TPROXY 拦截
iptables -w -t mangle -N ROG_TP      # 处理局域网进来的流量 (PREROUTING)
iptables -w -t mangle -N ROG_OUT     # 处理旁网关本机发出的流量 (OUTPUT)

# nat 表：负责 UDP 53 DNS 劫持与局域网直连流量出口 SNAT
iptables -w -t nat -N ROG_NAT        # 局域网直连流量出口源地址伪装 (MASQUERADE)
iptables -w -t nat -N ROG_DNS_PRE    # 劫持局域网客户端发出的 UDP 53 DNS 请求
iptables -w -t nat -N ROG_DNS_OUT    # 劫持旁网关本机其他进程发出的 UDP 53 DNS 请求
```

##### （3）`mangle` 表内网与本机白名单（`ROG_TP` 与 `ROG_OUT` 共用前 11 条）
确保发往旁网关本机（如 SSH `22` 端口）、DNS 查询以及内网设备互访流量**第一时间退出，绝不进入 TPROXY**：
```bash
# 1. 目标地址是旁网关本机自身 IP（如 SSH 连接 192.168.0.71:22）→ 直接 RETURN 放行
iptables -w -t mangle -A ROG_TP/ROG_OUT -m addrtype --dst-type LOCAL -j RETURN

# 2. 目标端口是 UDP 53（DNS 交由 nat 表重定向到 rog :53，不走 TPROXY）→ 直接 RETURN 放行
iptables -w -t mangle -A ROG_TP/ROG_OUT -p udp --dport 53 -j RETURN

# 3~11. 目标地址属于保留地址、环回地址、局域网私有网段或组播地址 → 直接 RETURN 放行
iptables -w -t mangle -A ROG_TP/ROG_OUT -d 0.0.0.0/8 -j RETURN
iptables -w -t mangle -A ROG_TP/ROG_OUT -d 10.0.0.0/8 -j RETURN
iptables -w -t mangle -A ROG_TP/ROG_OUT -d 127.0.0.0/8 -j RETURN
iptables -w -t mangle -A ROG_TP/ROG_OUT -d 169.254.0.0/16 -j RETURN
iptables -w -t mangle -A ROG_TP/ROG_OUT -d 172.16.0.0/12 -j RETURN
iptables -w -t mangle -A ROG_TP/ROG_OUT -d 192.168.0.0/16 -j RETURN
iptables -w -t mangle -A ROG_TP/ROG_OUT -d 198.18.0.0/15 -j RETURN
iptables -w -t mangle -A ROG_TP/ROG_OUT -d 224.0.0.0/4 -j RETURN
iptables -w -t mangle -A ROG_TP/ROG_OUT -d 240.0.0.0/4 -j RETURN
```
*(IPv6 的 `ROG6_TP` / `ROG6_OUT` 同样跳过 `LOCAL`、`:53`、`::/128`、`::1/128`、`fc00::/7`、`fe80::/10`、`ff00::/8` 以及本机所在局域网 IPv6 前缀)*

##### （4）`ROG_OUT` 链（处理旁网关宿主机自身发出的流量）
Linux 内核不允许在 `OUTPUT` 链直接调用 `-j TPROXY`，因此通过给本机流量打上 `0x77` 标记，触发内核重路由（reroute）经 `lo` 口送入 `PREROUTING` 的 `ROG_TP`：
```bash
# 插到第 1 条：若数据包源 IP 不是本机（例如正在被旁网关三层转发的局域网客户端包），直接退出
iptables -w -t mangle -I ROG_OUT 1 -m addrtype ! --src-type LOCAL -j RETURN

# 防回环死循环：rog 进程自身 (UID=999) 及同组豁免服务 (GID=990) 发出的包直接 RETURN 直连出网
iptables -w -t mangle -A ROG_OUT -m owner --uid-owner 999 -j RETURN
iptables -w -t mangle -A ROG_OUT -m owner --gid-owner 990 -j RETURN

# 旁网关本机其他程序（如 curl、apt）发出的 TCP/UDP 包打上 0x77 标记，触发 2809 表经 lo 重路由
iptables -w -t mangle -A ROG_OUT -p tcp -j MARK --set-xmark 0x77/0xff
iptables -w -t mangle -A ROG_OUT -p udp -j MARK --set-xmark 0x77/0xff
```

##### （5）`ROG_TP` 链末尾（已建立连接快路 + eBPF 直连旁路 + TPROXY 透明拦截）
```bash
# 1-2. 【已建立 TPROXY 连接快路】：若该 TCP 包属于已在 rog (:12345) 建立好五元组连接的透明套接字 (--transparent --nowildcard)，
#      直接打标 0x77 并 ACCEPT，省去后续重复匹配，且保证已建立连接绝不被中途打断
iptables -w -t mangle -A ROG_TP -p tcp -m socket --transparent --nowildcard -j MARK --set-xmark 0x77/0xff
iptables -w -t mangle -A ROG_TP -p tcp -m socket --transparent --nowildcard -j ACCEPT

# 3. 【eBPF 直连快路旁路】：若网卡 TC ingress 的 eBPF 程序查表判定该目的 IP 走 Direct，已给包打上 0x78 标记，
#    在此直接 RETURN 退出 ROG_TP，不进 TPROXY，交由 Linux 内核原生三层路由直接转发
iptables -w -t mangle -A ROG_TP -m mark --mark 0x78/0xff -j RETURN

# 4-5. 【TPROXY 透明拦截】：其余需要由 rog 处理的 TCP / UDP 流量，打上 0x77 标记并透明重定向到本机 12345 端口
iptables -w -t mangle -A ROG_TP -p tcp -j TPROXY --on-port 12345 --tproxy-mark 0x77/0xff
iptables -w -t mangle -A ROG_TP -p udp -j TPROXY --on-port 12345 --tproxy-mark 0x77/0xff
```

##### （6）将 `ROG_TP` 与 `ROG_OUT` 挂载到系统 `mangle` 主链
```bash
# 从 eth0 进入、源 IP 属于局域网 192.168.0.0/23 的流量跳入 ROG_TP
iptables -w -t mangle -A PREROUTING -i eth0 -s 192.168.0.0/23 -j ROG_TP

# 从 lo 口重路由回来、带有 0x77 标记的本机 OUTPUT 流量也跳入 ROG_TP 由 TPROXY 接收
iptables -w -t mangle -A PREROUTING -i lo -m mark --mark 0x77/0xff -j ROG_TP

# 本机发出的流量跳入 ROG_OUT
iptables -w -t mangle -A OUTPUT -j ROG_OUT
```

##### （7）`nat` 表（UDP 53 DNS 透明劫持 + 局域网 IPv4 出口 `MASQUERADE`）
```bash
# 1. 局域网客户端发往任何外部 IP 的 UDP 53 DNS 请求，统一 DNAT 重定向到旁网关本机 192.168.0.71:53
iptables -w -t nat -A ROG_DNS_PRE -p udp --dport 53 ! -d 192.168.0.71/32 -j DNAT --to-destination 192.168.0.71:53

# 2. 旁网关本机发出的 UDP 53 DNS 请求：排除非本机包、排除 rog 自身 (UID 999 / GID 990) 后，也重定向到 192.168.0.71:53
iptables -w -t nat -A ROG_DNS_OUT -m addrtype ! --src-type LOCAL -j RETURN
iptables -w -t nat -A ROG_DNS_OUT -m owner --uid-owner 999 -j RETURN
iptables -w -t nat -A ROG_DNS_OUT -m owner --gid-owner 990 -j RETURN
iptables -w -t nat -A ROG_DNS_OUT -p udp --dport 53 ! -d 192.168.0.71/32 -j DNAT --to-destination 192.168.0.71:53

# 3. 当局域网客户端流量（如命中 eBPF 0x78 直连快路的包）由内核经 eth0 转发给主路由时，执行源地址伪装 (MASQUERADE)，
#    确保主路由将回包原路发回旁网关，防止局域网同网段非对称路由丢包
iptables -w -t nat -A ROG_NAT -s 192.168.0.0/23 -o eth0 -j MASQUERADE

# 4. 将上述 nat 链挂载到系统 PREROUTING / OUTPUT / POSTROUTING
iptables -w -t nat -A PREROUTING -i eth0 -s 192.168.0.0/23 -j ROG_DNS_PRE
iptables -w -t nat -A OUTPUT -j ROG_DNS_OUT
iptables -w -t nat -A POSTROUTING -j ROG_NAT
```

---

#### 4.3 停止时执行的命令逐行详解（`tproxy-down.sh`）

停止时**绝不执行全表 `iptables -F`**（避免误伤系统或 Docker 等其他服务的规则），仅精确拆除 `rog` 自建的跳转规则、自定义链与策略路由表：

```bash
# 1. 从系统主链 PREROUTING / OUTPUT / POSTROUTING 中移除跳向 ROG_* 的规则
while iptables -w -t mangle -D PREROUTING -i eth0 -s 192.168.0.0/23 -j ROG_TP 2>/dev/null; do :; done
while iptables -w -t mangle -D PREROUTING -i lo -m mark --mark 0x77/0xff -j ROG_TP 2>/dev/null; do :; done
while iptables -w -t mangle -D OUTPUT -j ROG_OUT 2>/dev/null; do :; done
while iptables -w -t nat -D PREROUTING -i eth0 -s 192.168.0.0/23 -j ROG_DNS_PRE 2>/dev/null; do :; done
while iptables -w -t nat -D OUTPUT -j ROG_DNS_OUT 2>/dev/null; do :; done
while iptables -w -t nat -D POSTROUTING -j ROG_NAT 2>/dev/null; do :; done

# 2. 清空 (-F) 并删除 (-X) rog 自建的 5 条自定义链
for chain in ROG_TP ROG_OUT; do
    iptables -w -t mangle -F "$chain" 2>/dev/null || true
    iptables -w -t mangle -X "$chain" 2>/dev/null || true
done
for chain in ROG_NAT ROG_DNS_PRE ROG_DNS_OUT; do
    iptables -w -t nat -F "$chain" 2>/dev/null || true
    iptables -w -t nat -X "$chain" 2>/dev/null || true
done

# 3. 删除 2809 号策略路由规则并清空 2809 号路由表
while ip -4 rule del pref 2809 fwmark 0x77/0xff lookup 2809 2>/dev/null; do :; done
ip -4 route flush table 2809 2>/dev/null || true
```
*(随后对 `ip6tables` 的 `ROG6_TP`、`ROG6_OUT`、`ROG6_DNS_PRE`、`ROG6_DNS_OUT` 以及 `ip -6 rule/route table 2809` 执行完全一致的对称清理)*

---

### 5. 局域网客户端接入方式

- **IPv4 设置**：将客户端（或主路由 DHCP 下发）的 **默认网关（Router）** 和 **DNS 服务器** 均指向旁网关的 IPv4 地址（例如 `192.168.0.71`）。
- **IPv6 设置**：
  - 由于默认已开启 `suppress_ipv6_ra = true`，客户端无需手动关闭 IPv6，也无需担心主路由下发的 IPv6 RA / RDNSS 劫持流量。
  - 若需要客户端通过 IPv6 网关发起流量，可将客户端的 **IPv6 路由器（Router）** 指向旁网关网卡的 `fe80::...` 链路本地地址（`ip -6 addr show` 中 `scope link` 的地址）；若客户端仅配置了 IPv4 网关，也可在 `[dns]` 中按需开启 `disable_aaaa = true`。

## 独立 UDP DNS 服务

在配置中增加 `[dns]` 即可启动独立 UDP DNS 服务（支持监听双栈 `"[::]:53"`），客户端直接把 DNS 指向 rog 所在机器。DNS 复用 `dns.router` 引用的现有业务规则，跳过纯 IP 规则并禁止 `domain_to_ip`；每条查询按域名匹配规则选择上游 DNS：
- 当命中的 connector 配置了专属 `dns`（且不同于 `dns.default_servers`，如国内直连 `Direct` 配置 `["114.114.114.114:53", "223.5.5.5:53"]`）时，通过该 connector 查询其专属 DNS；
- 当命中的代理 connector 未配置专属 `dns`（或与 `dns.default_servers` 相同）时，默认复用路由器 `default` 出口查询 `dns.default_servers`。
- 支持配置 `disable_aaaa = true` 屏蔽 `AAAA` 记录（直接返回空 `NOERROR` 响应），默认为 `false`。

参考 `dns_config_example.toml` 和 `full_config_example.toml`。本机测试示例监听 `127.0.0.1:5353`，可使用 `dig @127.0.0.1 -p 5353 example.com` 验证；生产环境通常监听 `"[::]:53"`。TPROXY 和 DNS 在同一进程使用同一个 `router` 时，会自动共享真實 IP（IPv4/IPv6）→ 域名映射表：
- TPROXY 收到客户端连接后，自动根据目标 IP 反查已学习的域名进行域名规则分流；
- 当客户端发起 IPv6 连接且命中的代理出口为非直连节点（如仅支持 IPv4 出站的远端代理服务器）时，自动将目标 IPv6 地址还原为学习到的域名发往远端代理节点解析连接。

代理节点域名自身的启动解析（`bootstrap_dns`）按以下优先级自动回退，无需在每个节点重复配置：
`connector.options.bootstrap_dns` $\rightarrow$ `dns.bootstrap_servers` $\rightarrow$ `connector.dns` $\rightarrow$ `dns.default_servers`。节点域名通过这些服务器直连解析（带内存缓存与失败旧值兜底），绕过系统 DNS 和业务 DNS 路由，gRPC 保留域名 TLS/SNI。

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
