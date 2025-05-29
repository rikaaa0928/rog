use crate::def::{config, RouterSet, RunAcceptor, RunAccStream, RunListener};
use crate::util::RunAddr;
use crate::proto::v2::pb::rog_v2_server::{RogV2, RogV2Server};
use crate::proto::v2::pb::{StreamReq, StreamRes};
use crate::stream::rogv2_server::{RogV2ServerStream, RogV2UdpSocket};
use std::collections::HashMap;
use std::io::{Error, ErrorKind};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::codegen::tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};
use log::{debug, error, info};

pub struct RogV2Listener {
    cfg: config::Listener,
    // router: Arc<dyn RouterSet>, // Removed
}

impl RogV2Listener {
    pub fn new(cfg: config::Listener) -> Self { // _router argument completely removed
        Self { cfg }
    }
}

#[async_trait::async_trait]
impl RunListener for RogV2Listener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        // 创建一个channel用于传递连接
        let (tx, rx) = mpsc::channel::<(RunAccStream, SocketAddr)>(128);
        
        let service = RogV2ServiceImpl {
            auth: self.cfg.pw.clone().unwrap_or_default(),
            // router: Arc::clone(&self.router), // Removed
            connection_tx: tx,
        };
        
        let addr_parsed = addr.parse()
            .map_err(|e| Error::new(ErrorKind::InvalidInput, format!("invalid address: {}", e)))?;
        
        // 在后台启动gRPC服务器
        let addr_str = addr.to_string();
        tokio::spawn(async move {
            info!("Starting RogV2 listener on {}", addr_str);
            
            if let Err(e) = tonic::transport::Server::builder()
                .add_service(RogV2Server::new(service))
                .serve(addr_parsed)
                .await
            {
                error!("RogV2 server error: {}", e);
            }
        });
        
        // 返回acceptor
        Ok(Box::new(RogV2Acceptor::new(rx)))
    }
}

struct RogV2ServiceImpl {
    auth: String,
    // router: Arc<dyn RouterSet>, // Removed
    connection_tx: mpsc::Sender<(RunAccStream, SocketAddr)>,
}

#[tonic::async_trait]
impl RogV2 for RogV2ServiceImpl {
    type streamStream = Pin<Box<dyn Stream<Item = Result<StreamRes, Status>> + Send>>;

    async fn stream(
        &self,
        request: Request<Streaming<StreamReq>>,
    ) -> Result<Response<Self::streamStream>, Status> {
        let mut stream = request.into_inner();
        let auth = self.auth.clone();
        
        // 创建响应通道
        let (tx, rx) = mpsc::channel::<Result<StreamRes, Status>>(128);
        
        // 存储每个srcID对应的处理器
        let handlers: Arc<RwLock<HashMap<String, StreamHandler>>> = Arc::new(RwLock::new(HashMap::new()));
        
        let handlers_clone = Arc::clone(&handlers);
        let connection_tx = self.connection_tx.clone();
        
        tokio::spawn(async move {
            let mut authenticated = false;
            
            while let Some(result) = stream.message().await.transpose() {
                match result {
                    Ok(req) => {
                        // 首次请求必须包含认证信息
                        if !authenticated {
                            if req.auth != auth {
                                error!("Authentication failed");
                                let _ = tx.send(Err(Status::unauthenticated("invalid auth"))).await;
                                break;
                            }
                            authenticated = true;
                        }
                        
                        // 处理请求
                        if let Err(e) = handle_request(req, &tx, &handlers_clone, &connection_tx).await {
                            error!("Error handling request: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Stream error: {}", e);
                        break;
                    }
                }
            }
            
            // 清理所有处理器
            handlers_clone.write().await.clear();
        });
        
        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(output_stream)))
    }
}

struct StreamHandler {
    tx: mpsc::Sender<StreamReq>,
    is_udp: bool,
}

async fn handle_request(
    req: StreamReq,
    response_tx: &mpsc::Sender<Result<StreamRes, Status>>,
    handlers: &Arc<RwLock<HashMap<String, StreamHandler>>>,
    connection_tx: &mpsc::Sender<(RunAccStream, SocketAddr)>,
) -> Result<(), Box<dyn std::error::Error>> {
    match req.cmd {
        1 => { // HANDSHAKE_REQ
            let src_id = req.src_id.clone();
            
            // 检查srcID是否已存在
            {
                let handlers_read = handlers.read().await;
                if handlers_read.contains_key(&src_id) {
                    let res = StreamRes {
                        payload: vec![],
                        dst_addr: String::new(),
                        dst_port: 0,
                        src_addr: String::new(),
                        src_port: 0,
                        udp: false,
                        cmd: 2, // HANDSHAKE_CONFLICT_SRC_ID
                        src_id: src_id.clone(),
                        addons: HashMap::new(),
                    };
                    response_tx.send(Ok(res)).await?;
                    return Ok(());
                }
            }
            
            // 创建新的处理器
            let (handler_tx, handler_rx) = mpsc::channel::<StreamReq>(128);
            
            // 根据请求类型创建相应的流
            if req.udp {
                // UDP处理 - 需要创建UDP socket并返回给accept队列
                // 这里我们创建一个UDP reader/writer对
                let (udp_tx, udp_rx) = mpsc::channel::<StreamReq>(128);
                let (resp_tx_udp, resp_rx_udp) = mpsc::channel::<StreamRes>(128);
                
                // 启动UDP处理任务
                let udp_handler_tx = handler_tx.clone();
                let udp_response_tx = response_tx.clone();
                let udp_src_id = src_id.clone();
                
                tokio::spawn(async move {
                    let mut handler_rx = handler_rx;
                    // 转发请求到UDP处理器
                    while let Some(req) = handler_rx.recv().await {
                        if let Err(e) = udp_tx.send(req).await {
                            error!("Failed to forward UDP request: {}", e);
                            break;
                        }
                    }
                });
                
                // 转发UDP响应
                tokio::spawn(async move {
                    let mut resp_rx_udp = resp_rx_udp;
                    while let Some(res) = resp_rx_udp.recv().await {
                        let tonic_res = Ok(res);
                        if let Err(e) = udp_response_tx.send(tonic_res).await {
                            error!("Failed to forward UDP response: {}", e);
                            break;
                        }
                    }
                });
                
                // 创建UDP reader/writer
                let reader = Box::new(crate::stream::rogv2_server::RogV2UdpReader::new(
                    udp_rx,
                    udp_src_id.clone(),
                ));
                let writer = Box::new(crate::stream::rogv2_server::RogV2UdpWriter::new(
                    resp_tx_udp,
                    udp_src_id.clone(),
                ));
                
                // 发送到accept队列
                let stream = RunAccStream::UDPSocket((reader, writer));
                let addr = SocketAddr::from(([127, 0, 0, 1], 0));
                if let Err(e) = connection_tx.send((stream, addr)).await {
                    error!("Failed to send UDP connection to acceptor: {}", e);
                    return Err(Box::new(e));
                }
            } else {
                // TCP处理
                let tcp_stream = RogV2ServerStream::new(
                    handler_rx,
                    response_tx.clone(),
                    src_id.clone(),
                    req.dst_addr.clone(),
                    req.dst_port as u16,
                );
                
                // 将TCP stream发送到accept队列
                let stream = RunAccStream::TCPStream(Box::new(tcp_stream));
                // 使用一个虚拟的地址，实际地址信息在stream中
                let addr = SocketAddr::from(([127, 0, 0, 1], 0));
                if let Err(e) = connection_tx.send((stream, addr)).await {
                    error!("Failed to send connection to acceptor: {}", e);
                    return Err(Box::new(e));
                }
            }
            
            // 注册处理器
            {
                let mut handlers_write = handlers.write().await;
                handlers_write.insert(src_id.clone(), StreamHandler {
                    tx: handler_tx,
                    is_udp: req.udp,
                });
            }
            
            // 发送握手成功响应
            let res = StreamRes {
                payload: vec![],
                dst_addr: String::new(),
                dst_port: 0,
                src_addr: String::new(),
                src_port: 0,
                udp: false,
                cmd: 1, // HANDSHAKE_DONE
                src_id,
                addons: HashMap::new(),
            };
            response_tx.send(Ok(res)).await?;
        }
        
        0 => { // DATA
            let src_id = req.src_id.clone();
            let handlers_read = handlers.read().await;
            
            if let Some(handler) = handlers_read.get(&src_id) {
                handler.tx.send(req).await?;
            } else {
                debug!("No handler found for srcID: {}", src_id);
            }
        }
        
        3 => { // CLOSE_SRC_ID
            let src_id = req.src_id.clone();
            handlers.write().await.remove(&src_id);
            debug!("Closed srcID: {}", src_id);
        }
        
        _ => {
            debug!("Unknown command: {}", req.cmd);
        }
    }
    
    Ok(())
}

// RogV2Acceptor实现，用于与现有的Object系统集成
pub struct RogV2Acceptor {
    rx: Arc<Mutex<mpsc::Receiver<(RunAccStream, SocketAddr)>>>,
}

impl RogV2Acceptor {
    pub fn new(rx: mpsc::Receiver<(RunAccStream, SocketAddr)>) -> Self {
        Self { rx: Arc::new(Mutex::new(rx)) }
    }
}

#[async_trait::async_trait]
impl RunAcceptor for RogV2Acceptor {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let mut rx = self.rx.lock().await;
        rx.recv().await
            .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "acceptor closed"))
    }

    async fn handshake(
        &self,
        r: &mut dyn crate::def::RunReadHalf,
        _w: &mut dyn crate::def::RunWriteHalf,
    ) -> std::io::Result<RunAddr> {
        // RogV2的握手已经在gRPC层面完成
        // 从stream中获取实际的地址信息
        if let Some((addr, _)) = r.handshake().await? {
            Ok(addr)
        } else {
            // 如果无法获取地址信息，返回错误
            Err(Error::new(ErrorKind::InvalidData, "No address information in RogV2 stream"))
        }
    }
}
