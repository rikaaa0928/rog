# 独立 UDP DNS 服务与真实 IP 域名映射设计

本文是后续实现的设计基准。采用独立 UDP DNS 转发服务，复用业务路由规则和 outbound，通过真实 DNS 响应学习域名关联，为 TPROXY 的域名分流提供依据。

## 1. 已确定的行为

1. 客户端主动把 DNS 指向 rog 的 DNS 监听地址；DNS 服务独立于 TPROXY 监听器。
2. 首版只支持客户端 UDP DNS 和上游 UDP DNS，不实现 TCP/53、DoH、DoT。
3. 不生成假 IP，不实现 FakeDNS，不缓存 DNS 响应，不实现递归解析器。
4. DNS 路由复用业务 router 的同一份 `route_rules`、顺序和默认 outbound，不增加独立 DNS 规则表。
5. DNS 根据请求中的域名选择 outbound，不能按客户端原 DNS 地址或上游 DNS IP 再做业务路由。
6. 每个 outbound 可以配置多个上游 DNS；命中规则后优先使用该 outbound 的列表，没有列表时使用 DNS 服务默认列表。
7. DNS 未命中任何规则时使用业务 router 的默认 outbound，服务器列表固定使用 DNS 服务的默认列表。
8. 业务连接使用目标 IP 和有效域名集合，按照用户配置顺序逐条匹配；没有强制 IP 优先。建议把局域网等 IP 直连规则放在前面。
9. 一个 IP 可以关联多个域名：先遍历规则，再遍历该规则的域名候选，第一条命中规则胜出。
10. DNS 响应先记录映射，再回复客户端。业务连接继续连接原始真实 IP，不把连接目标改成域名再解析。

### 1.1 首版边界

- 上游 DNS 配置只接受 IPv4 字面地址和端口，例如 `4.2.2.1:53`。这是复用当前 IPv4 UDP direct connector 的首版限制，不限制返回 AAAA 记录。
- DNS 监听器可以监听 IPv4 或 IPv6；IPv6 客户端地址必须正确格式化、归一化并保留。
- UDP 响应带 `TC=1` 时原样返回，不自动改用 TCP，不学习其中的部分地址。客户端可能回退 TCP，该路径不属于首版。
- 客户端已有 DNS 缓存、应用自带加密 DNS、其他 DNS 服务或未经过此服务的查询，不保证能建立映射。
- 普通 DNS 返回真实 IP；共享 IP 的域名归因是启发式的，不能证明某条业务连接实际访问了哪个域名。
- 不在首版实现外部 DNS 工具的映射同步、跨进程共享、映射持久化、上游 UDP tunnel 池、DNS 请求合并或响应缓存。
- 本设计只增加 DNS 服务与路由集成，不重新实现 TPROXY 基础设施。实现以 `feat/tproxy` 的 `14acee0` 基础提交为起点，复用其监听器、up/down 和退出管理。

## 2. 整体架构

```text
客户端 DNS 查询
      │ 直接查询 rog:53（开发时可用 :5353）
      ▼
独立 DnsServer
      ├─解析 QName、QType、QClass
      ├─复用配置的业务 router，纯域名匹配
      ├─选择 outbound 与上游 DNS 列表
      ├─通过该 outbound 的 udp_tunnel 查询上游
      ├─验证响应，提取相关 A/AAAA 与 CNAME 链
      ├─更新 IpDomainTable
      └─用监听 socket 回复客户端，保留真实 DNS 响应

客户端业务连接
      │ TPROXY 获取真实目标 IP 与客户端 IP
      ▼
router.route_with_context()
      ├─从同进程共享表查询域名集合
      ├─按原规则顺序匹配目标 IP 或候选域名
      └─命中后连接原始目标 IP，未命中走 router.default
```

DNS 服务使用正常 UDP `recv_from/send_to`，回复天然来自客户端配置的 DNS 服务地址。不需要 `IP_TRANSPARENT`、原目的 DNS 地址恢复、伪装回包 socket 或 DNS 防火墙劫持。

DNS 服务必须与消费映射的业务路由器运行在同一 rog 进程；独立监听器不代表另开一个进程。

## 3. 配置契约

### 3.1 顶层 DNS 服务配置

新增可选的 `Config.dns: Option<DnsConfig>`。缺少 `[dns]` 时不启动服务、不启用新映射路径，保持原有行为。

```rust
pub struct DnsConfig {
    pub endpoint: String,
    pub router: String,
    pub default_servers: Vec<String>,
    pub upstream_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub max_concurrent_requests: usize,
    pub mapping_capacity: usize,
    pub max_domains_per_ip: usize,
    pub mapping_max_ttl_secs: u64,
    pub mapping_cleanup_interval_secs: u64,
}
```

`dns.router` 引用现有业务 router，例如 TPROXY 的 `listener.router = "main"` 与 `dns.router = "main"`。这是指定服务使用哪个现有 router，不是一份独立 DNS 路由配置。

除 `endpoint`、`router`、`default_servers` 外均提供 serde 默认值：

| 字段 | 默认值 | 含义 |
|---|---:|---|
| `upstream_timeout_ms` | 2000 | 每台上游一次完整尝试的预算，包含建立 connector/tunnel、发送、接收 |
| `request_timeout_ms` | 6000 | 整个请求总预算，覆盖路由、所有尝试、解析与表更新 |
| `max_concurrent_requests` | 256 | 本 DNS 服务同时处理的请求上限 |
| `mapping_capacity` | 16384 | `(router, client_ip, target_ip)` 键的数量上限 |
| `max_domains_per_ip` | 16 | 每个键保留的域名记录上限 |
| `mapping_max_ttl_secs` | 86400 | 映射 TTL 上限，不提升 TTL 下限 |
| `mapping_cleanup_interval_secs` | 30 | 清理间隔；读取时仍必须过滤过期记录 |

这些默认值是初始工程配置，不是性能或容量承诺。所有超时和容量必须大于零。`default_servers` 必须非空。服务器按顺序使用，重复项在启动解析时去重、保留首次位置。

### 3.2 outbound DNS 配置

在 `Connector` 增加可选的 `dns: Option<Vec<String>>`。字段表示上游服务器列表，不改变 connector 的业务连接行为。

缺失、`None` 或空列表均视为“未配置专属 DNS”，使用默认服务器。任何非空列表中的非法地址都应在启动前报错，不能静默忽略。

现有 `RouteRule.dns` 是 `domain_to_ip` 使用的解析设置，与新增 `Connector.dns` 不同；不要改名、复用字段或改变旧语义。

### 3.3 上游与出口选择

| 路由结果 | outbound | 上游 DNS |
|---|---|---|
| 命中规则，所选 connector 的 `dns` 非空 | 所选 connector | connector 的列表 |
| 命中规则，所选 connector 没有 `dns` | 所选 connector | `dns.default_servers` |
| 未命中规则 | 同一个 router 的 `default` | `dns.default_servers`，不使用默认 connector 的专属列表 |
| 所选 connector 的协议为 `block` | 不发请求 | 本地返回 REFUSED |

判断 block 依据 `connector.proto`，不能要求 connector 的名称必须叫 `block`。

不增加 `dns_router` 规则表、不增加 `default_dns_connector`，也不额外路由 DNS 服务器 IP。一个请求的全部服务器尝试都使用同一个已选 outbound，禁止失败时静默切 direct。

## 4. DNS 路由语义

### 4.1 共享规则、独立匹配模式

在同一个命名 router 上提供 DNS 查询方法：

```rust
pub struct DnsRouteDecision {
    pub outbound: String,
    pub matched_rule: Option<String>,
}

async fn route_dns(&self, listener_name: &str, router_name: &str, qname: &str)
    -> std::io::Result<DnsRouteDecision>;
```

必须返回“是否命中规则”，不能仅返回 outbound 名称。规则显式选中了默认 outbound，仍算命中，可使用该 outbound 的专属 DNS；真正未命中才使用默认服务器列表。

匹配算法：

1. 使用 `dns.router` 找到已有 router；配置缺失或名字错误在启动前拒绝，不沿用现有隐式 `"default"` 标签兜底。
2. 按该 router 原有规则顺序遍历。
3. IP/CIDR/LAN 规则不参与 DNS QName 匹配。
4. `domain-suffix`、`domain-keyword`、`regex` 规则只匹配规范化后的 QName。
5. 对 QName 应用该规则的 `exclude`；被排除则继续下一条规则。
6. 强制忽略 `domain_to_ip` 及其 `RouteRule.dns`。不为 DNS 路由调用 Resolver。
7. 命中返回规则名和所选 outbound；否则返回同一 router 的默认 outbound、`matched_rule = None`。

QName 使用 DNS 名称的规范 ASCII 表示、大小写归一、去掉根末尾点；不做 suffix 以外的擅自截断。国际化名称保留其 punycode 形式。查询原始字节不因规范化而改写。

### 4.2 规则类型信息

当前 `Matcher` 只有 `match_host()`，无法区分规则的语义。实现时在编译后的路由数据上保留类型，例如 `Ip`、`Domain`、`Regex`。

- `cidr`、`lan` 属于 `Ip`。
- domain suffix/keyword 及其现有别名属于 `Domain`。
- `regex` 保持现有语义：业务匹配允许测试原始 IP 字符串和候选域名；DNS 匹配只测试 QName。
- 远程动态 matcher 即使尚未加载内容，也必须保留配置声明的类型，不依赖下载后字符串推断。

保留现有规则、别名、远程数据更新与 `exclude` 行为；不要为了本功能重写所有 matcher。

## 5. UDP 请求处理

### 5.1 收包与并发

DNS 服务有自己的接收循环，不经过 `Object` 的通用 `RunAcceptor` 或 `handle_raw_udp()`。否则首包固定出口逻辑会破坏 DNS 的逐请求分流。

- 使用 Tokio UDP socket；接收缓冲区 65535 字节，不使用 512 字节缓冲区截断带 EDNS 的报文。
- 每个有效请求独立获取并发 permit，并由受监督的任务执行。
- 接收循环先尝试获取 permit，再 spawn；没有 permit 时丢弃请求并计数，不启动无限数量的“等待 permit”任务，也不建立无界队列。
- 客户端重传是新的请求，首版不做去重或合并。
- 显式处理 shutdown：停止接收、取消在途任务、等待任务退出，释放 permit、socket 和 tunnel。

### 5.2 输入处理

使用直接声明的 `hickory-proto = "0.25.2"` 依赖进行报文解析，与当前锁文件中的版本对齐。不能依赖间接依赖被 Rust 自动导入。

首版接受：标准 QUERY、单个 Question、QClass IN。所有普通 QType 均可转发，不只处理 A/AAAA；地址学习只针对可确认的 A/AAAA。

| 输入 | 行为 |
|---|---|
| 正常单问题 QUERY/IN | 分流并转发 |
| `QR=1` 的客户端报文 | 丢弃，不创建任务或映射 |
| 非 QUERY opcode | 可安全取得报文头时返回 NOTIMP |
| Question 数量不为 1、格式损坏 | 可安全构造错误回复时返回 FORMERR，否则丢弃 |
| 非 IN class、AXFR/IXFR | 返回 REFUSED，不访问上游 |

错误回复保留可安全解析的客户端 ID 和 Question，设置 QR、正确 RCODE，保留 RD，清空 AD，不伪造 DNSSEC 验证结果；长度必须有界。

### 5.3 转发与 tunnel 生命周期

每个请求、每次上游尝试建立一个独立 UDP tunnel，发送原始 DNS 查询字节。首版不跨请求复用 tunnel，因此相同客户端 ID 不会在共享 reader 上串包。

通过 `RunConnector::udp_tunnel()` 发送 `UDPPacket`：

- `meta.src_addr/src_port` 是真实客户端地址，用于现有 connector 的上下文契约。
- `meta.dst_addr/dst_port` 是选定上游的 IPv4 字面地址与端口。
- `data` 是原始 DNS 报文，保留 EDNS、未知 RR 及 DNSSEC 相关字段。
- 客户端 endpoint 用 `SocketAddr::to_string()` 格式化，禁止无方括号拼接 IPv6。

复用 connector 对象允许保留其已有连接设施，但不能让本请求复用其他 DNS 请求的 datagram reader。读取当前协议实现确认契约：direct 的响应 metadata 中 `dst` 表示远端、`src` 表示客户端；不可按字段名称盲目交换。

connector 初始化与 `udp_tunnel()` 都放入超时和取消范围。共享 connector cache 锁不得跨越网络初始化 await；不要直接复制当前业务 cache 长时间持锁的模式。

收到响应时，若 connector 提供远端 metadata，应核对它是本次选定上游；同时检查 ID、QR、opcode 和完整 Question 的 QName/QType/QClass。无效响应不学习也不回复，在当前尝试剩余预算内继续等待。解析和循环均有界。

### 5.4 上游轮换与失败结果

1. 按服务器列表顺序尝试，每台一次，不在首版并行向多个 DNS 扩散查询。
2. 每次预算不超过 `upstream_timeout_ms`，且不得超过请求总 deadline 的剩余时间。
3. 建立连接失败、发送/读取错误、超时、上游 SERVFAIL/REFUSED，可继续列表下一台。
4. 有效 NOERROR（包含无地址的 NODATA）和 NXDOMAIN 属于已完成查询，立即返回，不换服务器。
5. 合法 `TC=1` 响应立即原样返回，不学习、不用另一个 DNS 绕过截断。
6. 其他有效 DNS 错误响应原样返回，不当作网络错误；完整遍历失败或总超时则构造 SERVFAIL。
7. 显式 block 返回 REFUSED，不建立任何上游连接。
8. 客户端 send 失败只记录并释放资源，不能因此重新查询上游。

收到有效回复通常返回原始字节，不用 `lookup_ip()` 重建响应；`lookup_ip()` 只解决地址解析，不能替代任意 DNS 请求的透明转发。

### 5.5 启动解析与循环依赖

宿主机系统 DNS 可能指向本服务。上游 DNS 使用字面 IP，因此其地址不需要再解析；代理节点本身的域名 endpoint 仍可能让 connector 调用系统解析器，形成 DNS 自依赖。

首版明确约束：DNS router 可达的代理 connector 使用字面 IP endpoint，或显式配置 `connector.options.bootstrap_dns` 字面 IP 列表。bootstrap 直接通过指定服务器解析节点，绕过系统 DNS 与业务 DNS router；gRPC URI 保留原域名以维持 TLS/SNI。示例使用字面 IP。不能把本机 DNS 服务作为自身上游，启动校验拒绝能直接识别的自身 endpoint（包括环回、已配置本机地址与同端口）。

域名 endpoint 的 bootstrap 查询不允许回退系统 DNS；没有配置 bootstrap 的域名 DNS 出口在启动时拒绝。DNS 查询、connector 初始化和业务 `domain_to_ip` 是不同路径。

## 6. 真实 IP 域名映射

### 6.1 表结构与作用域

进程内仅有一个共享 `Arc<IpDomainTable>`，由 DNS 服务写入、对应业务 router 读取。建议键为：

```rust
pub struct MappingKey {
    pub router: String,
    pub client_ip: IpAddr,
    pub target_ip: IpAddr,
}

pub struct DomainRecord {
    pub domain: String,
    pub expires_at: Instant,
    pub last_observed_at: Instant,
}
```

router 名称防止多个业务策略之间串用；不包含客户端端口，因为 DNS 查询与业务连接使用不同端口。IPv4-mapped IPv6 统一成 IPv4，在记录和读取路径使用同一个归一化函数。

客户端隔离默认严格开启，不在本客户端未命中时查询其他客户端的映射；没有客户端 IP 就不使用映射。重启后为空表，新的 DNS 查询重新学习，不持久化。

### 6.2 地址提取与 CNAME

- 只从与原 QName 对应的有效应答链提取 A/AAAA；不能把整个报文中的所有地址都归入 QName。
- 原 QName 直接有地址时，关联 QName 与该地址。
- 存在 `QName -> CNAME -> ... -> owner -> A/AAAA` 时关联原 QName 与最终地址。
- 首版只记录客户端查询的 QName，不把每个 CNAME 别名自动加入业务路由候选。
- CNAME 链最多 16 跳，并检测循环；循环、断链或超出上限时不学习相应链，仍可以返回已经校验有效的原始 DNS 响应。
- 忽略 Authority 和无关 Additional 地址；MX、NS、HTTPS/SVCB hints 等不是首版地址学习来源。其他类型继续原样转发。
- NXDOMAIN、NODATA、TC、失败响应不新增地址，也不立即清空先前尚有效的正向关联。

### 6.3 TTL 和更新

对每个最终地址分别计算相关 CNAME 链与地址记录的最小 TTL，再与 `mapping_max_ttl_secs` 取最小值。

- TTL=0 不记录长期映射。
- 不把短 TTL 提升到 5 秒或其他下限。
- 有效期使用 monotonic `Instant`，从本次响应接收时刻计算，不在实际写表较晚时重新起算。
- 同一键同一域名去重；多次有效观测时保留尚有效记录与新观测中较晚的到期时刻。这表示此前已返回给客户端的 DNS 结果仍可能有效，不是任意延长单次 DNS TTL。
- 新响应中没有某个旧 IP，不立即删除旧关联；旧客户端缓存可能仍在使用它，按原到期时间淘汰。
- 读取时同步过滤过期记录，定时清理仅用于释放空间。

### 6.4 容量和时序

首版可使用一个短时持有的 mutex 与带顺序的有界表，避免仅有 `DashMap` 却无法严格控制容量。表操作不得包含网络 await。

- 全表最多 `mapping_capacity` 个键；插入前清理过期项，仍满则淘汰最久未观测的键。
- 每键最多 `max_domains_per_ip` 个域名；先去重和清理，再淘汰最久未观测的域名。
- 记录、容量检查与淘汰必须在同一个原子临界区完成，防止并发突破上限。
- 容量策略允许丢失域名关联，不影响真实 DNS 返回；表更新结果和淘汰计数应可观察。

正常流程先完成整个响应的地址学习，再 `send_to()` 回复。TTL=0、无可学习地址、容量淘汰、已有客户端缓存等情况仍可能查不到映射，不承诺所有连接都能识别域名。

## 7. 业务路由集成

### 7.1 保留原接口、增加上下文

当前 `RunAddr` 和 `RouterSet` 被多个入站及测试使用。为减少无关改动，保留旧 `route()` 与 `RunAddr`，新增带上下文的方法：

```rust
pub struct RouteContext {
    pub client_ip: Option<IpAddr>,
    pub use_dns_mapping: bool,
}

async fn route_with_context(
    &self,
    listener_name: &str,
    router_name: &str,
    addr: &RunAddr,
    context: &RouteContext,
) -> String;
```

trait 可为新方法提供委托旧 `route()` 的默认实现，保持已有 mock/第三方实现可用；实际 `DefaultRouter` 实现映射逻辑。`route_dns()` 同样加到顶层 `RouterSet`，默认返回 Unsupported，不能用普通 `route()` 悄悄替代其语义。

仅 TPROXY 的真实 IP 业务连接显式开启 `use_dns_mapping`。普通 SOCKS/HTTP/gRPC 等入站默认沿用原路径，避免给非目标功能引入新语义。

TCP 上下文来自真实 peer IP，UDP 上下文来自首包 `UDPMeta.src_addr`；不得用上游 socket 地址、代理节点 IP 或监听地址代替。一个 UDP 会话路由一次，不在后续 packet 或映射更新时切换已经建立的出口。

### 7.2 按用户配置顺序匹配

对 TPROXY 的真实目标 IP：

```text
domains = table.get_domains(router_name, normalized_client_ip, target_ip)
for rule in router.rules 的原始顺序:
    Ip rule: 对 target_ip 做匹配和 exclude
    Domain rule: 对 domains 逐个做匹配和 exclude
    Regex rule: 先测试 target_ip，再测试 domains，各自应用 exclude
    任一候选命中当前规则 -> 返回该规则的 outbound
没有规则命中 -> router.default
```

规则顺序永远比域名顺序优先。域名排序可以确定化以便日志和测试稳定，但不影响选择。某个域名被当前规则排除，只跳过该域名，其他域名仍可命中；IP 候选被排除也不自动排除域名候选。

例：IP 关联 `{a.com, b.com, c.com}`，规则依次为 `c.com -> C`、`b.com -> B`，选择 C。若前面有匹配该 IP 的直连规则则选择直连；若 IP 规则写在后面，就不能覆盖已经命中的域名规则。

业务路由通过映射选择 outbound 后仍连接 `target_ip:port`。禁止把 IP 替换为候选域名，否则重新解析可能改变真实连接目标。

### 7.3 `domain_to_ip` 兼容

- 已经以域名为目标的旧入站路径保留现有 `domain_to_ip` 与 `RouteRule.dns` 行为。
- TPROXY 映射路径已持有真实目标 IP，不额外把所有候选域名再解析；IP 规则只测试实际连接目标。
- DNS Query 路由完全禁止 `domain_to_ip`。
- 某个域名通过 DNS router 选到 C，不保证业务也选 C：业务可能先命中 IP 规则或共享 IP 上其他域名的较早规则。这是两种输入按同一配置计算的预期差异。

## 8. 配置示例

以下是设计后的最小示例；新字段在实现前不能直接用于当前二进制。代理 endpoint 使用文档保留地址作为占位符，部署时替换为实际节点 IP 和密码。

```toml
[dns]
endpoint = "0.0.0.0:53"
router = "main"
default_servers = ["4.2.2.1:53", "1.1.1.1:53"]
upstream_timeout_ms = 2000
request_timeout_ms = 6000
max_concurrent_requests = 256
mapping_capacity = 16384
max_domains_per_ip = 16
mapping_max_ttl_secs = 86400
mapping_cleanup_interval_secs = 30

[[listener]]
name = "tproxy_in"
endpoint = "0.0.0.0:12345"
proto = "tproxy"
router = "main"
# TPROXY 的 mode、up/down 与单监听器限制沿用其已确认的基础实现。

[[connector]]
name = "direct"
proto = "tcp"
dns = ["4.2.2.1:53"]

[[connector]]
name = "proxy"
proto = "pb_tcp"
endpoint = "192.0.2.10:9290"
pw = "replace-with-your-password"
dns = ["8.8.8.8:53", "1.1.1.1:53"]

[[connector]]
name = "deny"
proto = "block"

[[data]]
name = "lan"
format = "cidr"
data = """
10.0.0.0/8
172.16.0.0/12
192.168.0.0/16
127.0.0.0/8
"""

[[data]]
name = "proxy_domains"
format = "domain-suffix"
data = """
example.com
example.org
"""

[[data]]
name = "blocked_domains"
format = "domain-suffix"
data = "blocked.example"

[[router]]
name = "main"
default = "direct"

# 建议把 IP 直连规则放前面；顺序完全由用户控制。
[[router.route_rules]]
name = "lan"
select = "direct"
exclude = []

[[router.route_rules]]
name = "blocked_domains"
select = "deny"
exclude = []

[[router.route_rules]]
name = "proxy_domains"
select = "proxy"
exclude = []
```

DNS 查询 `example.com` 跳过 LAN 规则，选 proxy 和其专属 DNS；查询未匹配域名走 direct 和 `[dns].default_servers`，不是 direct 的专属列表。业务访问 LAN IP 优先命中前面的 LAN 规则。

## 9. 部署与观测

- 客户端 DNS 地址指向 rog 所在机器的 LAN IP；同机客户端可指向 `127.0.0.1`。开发使用 :5353 和测试客户端显式指定端口，系统 DNS 通常需要 :53。
- 若网关先代理 DNS，再转发至 rog，rog 通常只看到网关 IP，而业务连接仍来自原客户端，严格客户端隔离会使映射查不到。推荐 DHCP 下发 rog 地址、客户端直接查询 rog；首版不增加全局映射兜底来掩盖这个问题。
- TPROXY/防火墙必须让发往本机 DNS 服务的流量正常到达 DNS socket；该地址不能误入业务透明转发，rog 发出的上游查询和代理节点连接不能再次被拦截。沿用现有出站豁免，验证实际规则而非只检查端口。
- DNS listener 与系统已有 DNS 服务的 :53 占用冲突，应在 bind 时明确报错；不要自动关闭系统服务。
- DNS 接收、选择出口、上游尝试、结果、耗时、映射学习/淘汰/未命中、并发拒绝分别计数。日志记录实际命中规则、是否走默认和选定上游，默认不打印原始 DNS 报文或认证参数。
- 所有临时请求资源在成功、失败、超时和取消路径释放；共享清理任务随服务退出结束。

## 10. 文件与接口实施规划

| 文件/模块 | 工作 |
|---|---|
| `src/def/config.rs` | 可选顶层 DnsConfig、Connector.dns、默认值与启动校验 |
| `Cargo.toml` / `Cargo.lock` | 直接声明现有版本 hickory-proto，不顺带升级无关依赖 |
| `src/dns/config.rs` | 配置解析为 SocketAddr、去重、已选 outbound 与上游选择 |
| `src/dns/message.rs` | QueryContext、响应验证、受限 CNAME 地址提取、错误响应构造 |
| `src/dns/table.rs` | 客户端/router 隔离、有界映射、TTL 和清理 |
| `src/dns/server.rs` | UDP socket、并发 permit、逐请求 tunnel、超时与 shutdown |
| `src/def.rs` | RouteContext、DnsRouteDecision 与 RouterSet 新方法 |
| `src/router.rs` / `default_router.rs` | 共享表、新匹配模式与逐规则域名集合匹配 |
| `src/router/data.rs` / `remote_matcher.rs` | 保留数据类型并维持现有动态更新行为 |
| `src/object.rs` / `src/object/raw_udp.rs` | TPROXY 业务请求准确传递 client IP 和启用标志；原入站默认不变 |
| `src/lib.rs` | 一次创建共享表，将 DNS 服务、路由器、清理任务纳入运行与退出管理 |
| `src/listener/tproxy.rs`（基础功能合入后） | 只接业务，不新增 DNS 嗅探、53 端口独立分流或身份恢复逻辑 |

启动先完成配置验证，再 bind DNS listener/启动业务服务。验证已有 router 与 connector 引用、端口和地址、DNS 上游列表、预算容量、代理 DNS 出口启动解析约束。未知协议或无 UDP 能力不能默认为 direct；具体运行时 `udp_tunnel()` 返回 None/错误时按请求失败处理。

DNS 服务启动失败应结束本次运行并停止其他已启动 listener，不让用户误以为学习路径可用。需要注意当前 `run()` 只等待普通 Object 任务，后续应统一取消并等待全部 DNS/业务任务，不能只添加一个未监督的 spawn。

## 11. 实施顺序

1. **确定基础版本**：确认 TPROXY 基础入口可用，检查工作区，保留其他改动。缺少基础入口时报告依赖状态，不暗中把旧劫持方案恢复回来。
2. **配置与共享规则模式**：实现 DnsConfig/Connector.dns、验证与带命中标记的 `route_dns()`，先测默认选择表和顺序。
3. **报文与映射工具**：实现纯解析/提取函数和受控时间的有界映射测试，不接公网 DNS。
4. **DNS 转发服务**：本地 mock 上游、mock outbound 完成请求独立分流、响应还原、轮换、超时和取消。
5. **业务集成**：同一个表接入 TPROXY 上下文；保留旧路由路径；确认只是选出口、不改变真实目标 IP。
6. **运行管理与部署测试**：服务 bind 失败、shutdown、Linux 编译检查、本机 UDP 测试，再用已授权 Linux 环境完成 TPROXY 端到端验证。

## 12. 必须通过的验收场景

### 12.1 路由与配置

- 不配置 `[dns]` 时所有旧路由测试继续通过。
- DNS 与业务共享同一规则对象及顺序；DNS 跳过 CIDR，禁用 domain_to_ip。
- 命中带专属 DNS、命中不带 DNS、未命中但默认 connector 带 DNS，分别严格符合第 3.3 节选择表。
- 同一客户端 UDP socket 连续查询两个域名，分别经两个 outbound；不能被首包锁死。
- 两个客户端同时使用相同 DNS ID 查询相同/不同域名，不串包。
- 显式选择默认 outbound 的规则仍视为命中，与无规则命中可区分。
- connector 协议 block 返回 REFUSED，记录上游发送次数为零。
- 非法地址、空默认列表、错误 router/connector 引用和零容量在任何 socket/任务启动前报错。

### 12.2 DNS 协议与故障

- A/AAAA、NODATA、NXDOMAIN、非地址 QType、EDNS 等请求正常转发，原始成功响应保持不变。
- 第一个上游超时或返回 SERVFAIL，第二个成功；NXDOMAIN 不尝试第二个；全失败返回 SERVFAIL。
- 响应 ID、QName、QType、QClass、QR、opcode 或远端不匹配，不回复、不污染映射。
- 超时预算包括慢 connector 初始化，列表尝试不能超过总 deadline。
- 多个无效回复与畸形压缩/CNAME 链不会 panic、无限循环或无限扩容。
- TC 响应返回原报文、表无新增记录；TCP/DoH/DoT 没有被错误宣称支持。

### 12.3 映射与业务路由

- 同一 IP 关联 a/b/c，c 规则在 b 前，选择 c；域名输入顺序改变不影响结果。
- IP 规则在域名规则前/后分别得到相应结果，证明没有隐藏 IP 优先。
- 一个候选被 exclude，另一个候选仍能命中同一规则。
- 不同客户端或不同 router 的映射不串用；mapped IPv6 和 IPv4 地址一致处理。
- CNAME 多跳按每个地址链最小 TTL 学习原 QName，无关 Additional 地址不学习。
- TTL=0 不学习、短 TTL 不提升、读取立即过滤到期记录；容量和每键数量在并发插入下严格受限。
- 客户端收到 DNS 回复立即建立业务连接时，在没有过期/淘汰的正常路径可查到映射。
- 映射只影响新连接；已建立 TCP/UDP 不因为表更新切出口；实际连接目标仍是原 IP。

### 12.4 资源与真实部署

- 达到并发上限后任务数不再增长；超时/错误/取消后 permits 与请求 tunnel 均释放。
- 退出能终止 DNS、表清理和业务 listener；端口能重新绑定。
- 宿主机 DNS 指向 rog、代理节点使用独立固定 IP 时，首次代理 DNS 查询无循环依赖。
- 验证 LAN 客户端直连 DNS 服务和立即业务连接；检查真正经过指定 outbound，而不只是日志选中了标签。
- 检查 Windows/macOS 的独立 DNS 代码编译，以及 Linux TPROXY 基础集成编译。模拟测试、跨平台编译与真实 Linux 转发分别报告，不能互相替代。

验收不要求首版识别全部连接的域名；必须保证行为有界、回退明确、共享规则顺序稳定，以及真实连接目标不被学习表改变。
