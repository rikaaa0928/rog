# RogV2 协议实现文档

## 概述

RogV2 是一个基于 gRPC 的代理协议，支持在单个 gRPC stream 上复用多个 TCP 和 UDP 连接。这种设计减少了连接开销，提高了效率。

## 核心特性

1. **连接复用**：多个 TCP/UDP 连接共享同一个 gRPC stream
2. **唯一标识**：每个连接使用 UUID 生成的 srcID 进行标识
3. **数据分流**：服务端根据 srcID 正确路由数据包
4. **认证机制**：支持基于密码的认证
5. **错误处理**：包含 srcID 冲突检测和超时处理

## 协议设计

### 消息格式

```protobuf
message StreamReq {
  string auth = 1;          // 认证信息
  bytes payload = 2;        // 数据载荷
  string dstAddr = 3;       // 目标地址
  uint32 dstPort = 4;       // 目标端口
  string srcAddr = 5;       // 源地址（UDP）
  uint32 srcPort = 6;       // 源端口（UDP）
  bool udp = 7;             // 是否为UDP连接
  uint32 cmd = 8;           // 命令类型
  string srcID = 9;         // 连接唯一标识
  map<string, string> addons = 10; // 扩展字段
}
```

### 命令类型

- `0`: DATA - 数据传输
- `1`: HANDSHAKE_REQ - 握手请求
- `1`: HANDSHAKE_DONE - 握手成功（响应）
- `2`: HANDSHAKE_CONFLICT_SRC_ID - srcID 冲突（响应）
- `3`: CLOSE_SRC_ID - 关闭连接

## 工作流程

### 客户端流程

1. **建立 gRPC 连接**：客户端连接到服务器的 gRPC endpoint
2. **连接复用**：检查是否已有到目标服务器的 stream，如果有则复用
3. **生成 srcID**：为新连接生成唯一的 UUID 作为 srcID
4. **发送握手请求**：包含认证信息和目标地址
5. **等待握手响应**：确认连接建立成功
6. **数据传输**：使用 srcID 标识的连接进行数据传输

### 服务端流程

1. **监听 gRPC 连接**：在指定端口监听 gRPC 请求
2. **认证验证**：验证客户端提供的认证信息
3. **处理握手请求**：
   - 检查 srcID 是否冲突
   - 创建对应的 TCP/UDP 处理器
   - 注册 srcID 到处理器的映射
4. **数据分流**：根据 srcID 将数据路由到正确的连接
5. **连接管理**：处理连接关闭和清理

## 配置示例

```toml
# 服务端配置
[[listener]]
name = "rogv2_server"
endpoint = "0.0.0.0:8443"
proto = "rogv2"
pw = "your_auth_password"
router = "default_router"

# 客户端配置
[[connector]]
name = "rogv2_client"
endpoint = "https://your-server.com:8443"
proto = "rogv2"
pw = "your_auth_password"
```

## 实现细节

### StreamManager

客户端使用 `StreamManager` 管理到每个服务器的 gRPC stream：

- 维护 srcID 到回调通道的映射
- 处理响应分发
- 管理连接生命周期

### 连接标识

每个连接使用 UUID v4 生成唯一的 srcID，确保：

- 连接的唯一性
- 数据包的正确路由
- 避免连接冲突

### 错误处理

- **认证失败**：返回 `Status::unauthenticated`
- **srcID 冲突**：返回 `HANDSHAKE_CONFLICT_SRC_ID`
- **超时处理**：握手请求 10 秒超时
- **连接断开**：自动清理相关资源

## 性能优势

1. **减少连接数**：多个连接复用一个 gRPC stream
2. **降低延迟**：避免频繁的连接建立
3. **资源效率**：减少系统资源消耗
4. **简化管理**：统一的连接管理机制

## 注意事项

1. **UDP 处理**：UDP 连接需要特殊处理，目前简化了目标服务器的确定逻辑
2. **地址信息**：TCP 连接的实际地址信息在 stream 创建时确定
3. **并发安全**：使用 `Arc<RwLock>` 确保并发访问的安全性
4. **资源清理**：连接断开时自动清理相关资源

## 未来改进

1. 支持更复杂的路由策略
2. 添加流量控制机制
3. 实现连接池管理
4. 支持更多的认证方式
5. 添加性能监控和统计
