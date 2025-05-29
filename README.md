# rog

A flexible and powerful network proxy.

## Features

- **Multi-protocol support:** Supports various protocols including TCP, HTTP, SOCKS5, and gRPC.
- **Configurable routing:** Allows flexible routing rules based on various criteria.
- **Asynchronous architecture:** Built with Tokio for high performance and non-blocking operations.
- **Centralized configuration:** Configuration is managed through a TOML file.

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

The configuration is managed through a TOML file. Here's an example structure:

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

- `endpoint`: The address and port to listen on.
- `name`: A unique name for the listener.
- `proto`: The protocol to use (e.g., "tcp", "http", "socks5", "grpc").
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
- `proto`: The protocol of the connector (e.g., "tcp", "grpc").

### RogV2 Configuration Example

The RogV2 protocol allows multiplexing TCP and UDP connections over a single gRPC stream. Here's an example configuration:

```toml
# RogV2 Config Example
# This configuration demonstrates how to use the new rogv2 protocol,
# which supports multiplexing TCP and UDP connections over the same gRPC stream.

[[listener]]
name = "rogv2_server"
endpoint = "0.0.0.0:8443"  # gRPC service listening address
proto = "rogv2"
pw = "your_auth_password"  # Authentication password
router = "default_router"

[[connector]]
name = "rogv2_client"
endpoint = "https://your-server.com:8443"  # gRPC server address
proto = "rogv2"
pw = "your_auth_password"  # Authentication password

# Routing configuration
[[router]]
name = "default_router"
default = "rogv2_client"

# Usage Example:
# 1. After the server starts, it will listen for gRPC connections on port 8443.
# 2. After the client connects to the server, it will automatically multiplex multiple TCP/UDP connections
#    over the same gRPC stream.
# 3. Each connection has a unique srcID identifier to ensure correct data packet routing.

# Features:
# - TCP and UDP connections are multiplexed over the same gRPC stream, reducing connection overhead.
# - Uses srcID for connection identification and data packet routing.
# - Supports authentication mechanisms.
# - Supports connection handshake and conflict detection.
```

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
