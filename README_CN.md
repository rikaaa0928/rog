# rog

一个灵活且强大的网络代理。

## 特性

- **多协议支持:** 支持多种协议，包括 TCP、HTTP、SOCKS5 和 gRPC。
- **可配置路由:** 允许基于各种标准进行灵活的路由规则配置。
- **异步架构:** 使用 Tokio 构建，实现高性能和非阻塞操作。
- **集中式配置:** 通过 TOML 文件管理配置。

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

配置通过 TOML 文件进行管理。以下是一个示例结构：

```toml
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

- `endpoint`: 监听的地址和端口。
- `name`: 监听器的唯一名称。
- `proto`: 使用的协议 (例如, "tcp", "http", "socks5", "grpc")。
- `router`: 此监听器使用的路由器的名称。

#### `router`

- `name`: 路由器的唯一名称。
- `default`: 要使用的默认路由。
- `route_rules`: 路由规则列表。

#### `router.route_rules`

- `name`: 规则的唯一名称。
- `select`: 要匹配的模式。
- `exclude`: 要排除的模式列表。
- `domain_to_ip`: 是否将域名解析为 IP 地址。

#### `data`

- `name`: 数据源的唯一名称。
- `url`: 用于加载数据的可选 URL。
- `format`: 数据格式。
- `data`: 内联数据。

#### `connector`

- `endpoint`: 连接器的端点。
- `name`: 连接器的唯一名称。
- `user`: 用于身份验证的可选用户名。
- `pw`: 用于身份验证的可选密码。
- `proto`: 连接器的协议 (例如, "tcp", "grpc")。

### RogV2 配置示例

RogV2 协议允许在单个 gRPC 流上复用 TCP 和 UDP 连接。这是一个配置示例：

```toml
# RogV2 配置示例
# 这个配置展示了如何使用新的rogv2协议，它支持在同一个gRPC stream上复用TCP和UDP连接

[[listener]]
name = "rogv2_server"
endpoint = "0.0.0.0:8443"  # gRPC服务监听地址
proto = "rogv2"
pw = "your_auth_password"  # 认证密码
router = "default_router"

[[connector]]
name = "rogv2_client"
endpoint = "https://your-server.com:8443"  # gRPC服务端地址
proto = "rogv2"
pw = "your_auth_password"  # 认证密码

# 路由配置
[[router]]
name = "default_router"
default = "rogv2_client"

# 使用示例：
# 1. 服务端启动后会监听8443端口的gRPC连接
# 2. 客户端连接到服务端后，会自动复用同一个gRPC stream处理多个TCP/UDP连接
# 3. 每个连接都有唯一的srcID标识，确保数据包正确分流

# 特性：
# - TCP和UDP连接复用同一个gRPC stream，减少连接开销
# - 使用srcID进行连接标识和数据包分流
# - 支持认证机制
# - 支持连接握手和冲突检测
```

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
