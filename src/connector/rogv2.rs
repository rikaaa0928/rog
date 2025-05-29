use crate::def::{config, RunConnector, RunStream, RunUdpReader, RunUdpWriter, UDPPacket};
use crate::proto::v2::pb::rog_v2_client::RogV2Client;
use crate::proto::v2::pb::{StreamReq, StreamRes};
use crate::stream::rogv2_client::{RogV2ClientRunStream, RogV2UdpReader, RogV2UdpWriter};
use log::{debug, error};
use std::collections::HashMap;
use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tonic::codegen::tokio_stream;
use tonic::{server, Request};
use uuid::Uuid;

pub struct RogV2Connector {
    client: Arc<Mutex<RogV2Client<tonic::transport::Channel>>>,
    cfg: config::Connector,
    // 存储活跃的stream，key是目标服务器地址
    active_streams: Arc<RwLock<HashMap<String, Arc<StreamManager>>>>,
}

struct StreamManager {
    tx: mpsc::Sender<StreamReq>,
    rx: Arc<Mutex<tonic::Streaming<StreamRes>>>,
    // 存储srcID到回调的映射
    callbacks: Arc<RwLock<HashMap<String, mpsc::Sender<StreamRes>>>>,
    auth: String,
}

impl RogV2Connector {
    pub async fn new(cfg: &config::Connector) -> io::Result<Self> {
        let client = RogV2Client::connect(cfg.endpoint.as_ref().unwrap().clone())
            .await
            .map_err(|e| io::Error::new(ErrorKind::InvalidData, format!("connect error: {}", e)))?;

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            cfg: cfg.clone(),
            active_streams: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    async fn get_or_create_stream(&self, server_addr: &str) -> io::Result<Arc<StreamManager>> {
        // 先尝试获取现有的stream
        {
            let streams = self.active_streams.read().await;
            if let Some(stream) = streams.get(server_addr) {
                debug!("getting new rog_v2 stream for {}", server_addr);
                return Ok(Arc::clone(stream));
            }
        }

        // 创建新的stream
        let (tx, rx) = mpsc::channel::<StreamReq>(128);
        let rx_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let request = Request::new(rx_stream);
        debug!("creating new rog_v2 stream for {}", server_addr);
        let response = self
            .client
            .lock()
            .await
            .stream(request)
            .await
            .map_err(|e| io::Error::new(ErrorKind::Other, format!("grpc stream error: {}", e)))?;

        let resp_stream = response.into_inner();

        let stream_manager = Arc::new(StreamManager {
            tx,
            rx: Arc::new(Mutex::new(resp_stream)),
            callbacks: Arc::new(RwLock::new(HashMap::new())),
            auth: self.cfg.pw.clone().unwrap_or_default(),
        });

        // 启动响应处理任务
        let manager_clone = Arc::clone(&stream_manager);
        tokio::spawn(async move {
            Self::handle_responses(manager_clone).await;
        });

        // 存储stream
        {
            let mut streams = self.active_streams.write().await;
            streams.insert(server_addr.to_string(), Arc::clone(&stream_manager));
        }

        Ok(stream_manager)
    }

    async fn handle_responses(manager: Arc<StreamManager>) {
        let mut rx = manager.rx.lock().await;

        loop {
            match rx.message().await {
                Ok(Some(response)) => {
                    let src_id = response.src_id.clone();
                    if !src_id.is_empty() {
                        let callbacks = manager.callbacks.read().await;
                        if let Some(callback) = callbacks.get(&src_id) {
                            if let Err(_) = callback.send(response).await {
                                // 通道已关闭，从callbacks中移除
                                drop(callbacks); // 释放读锁
                                let mut callbacks_write = manager.callbacks.write().await;
                                callbacks_write.remove(&src_id);
                                debug!("Removed closed callback for srcID: {}", src_id);
                            }
                        } else {
                            debug!("No callback found for srcID: {}", src_id);
                        }
                    }
                }
                Ok(None) => {
                    // Stream ended
                    debug!("Response stream ended");
                    break;
                }
                Err(e) => {
                    error!("Stream error: {}", e);
                    break;
                }
            }
        }

        // 清理所有回调
        manager.callbacks.write().await.clear();
        debug!("Cleared all callbacks");
    }

    fn generate_src_id() -> String {
        Uuid::new_v4().to_string()
    }
}

#[async_trait::async_trait]
impl RunConnector for RogV2Connector {
    async fn connect(&self, addr: String) -> io::Result<Box<dyn RunStream>> {
        let (host, port) = parse_address(&addr)?;
        let server_addr = format!("{}:{}", host, port);

        // 获取或创建到目标服务器的stream
        let stream_manager = self.get_or_create_stream(&server_addr).await?;

        // 生成唯一的srcID
        let src_id = Self::generate_src_id();

        // 创建用于接收此连接响应的channel
        let (resp_tx, mut resp_rx) = mpsc::channel::<StreamRes>(128);

        // 注册回调
        {
            let mut callbacks = stream_manager.callbacks.write().await;
            callbacks.insert(src_id.clone(), resp_tx);
        }

        // 发送握手请求
        let handshake_req = StreamReq {
            auth: stream_manager.auth.clone(),
            payload: vec![],
            dst_addr: host.clone(),
            dst_port: port as u32,
            src_addr: String::new(),
            src_port: 0,
            udp: false,
            cmd: 1, // HANDSHAKE_REQ
            src_id: src_id.clone(),
            addons: HashMap::new(),
        };

        stream_manager.tx.send(handshake_req).await.map_err(|e| {
            io::Error::new(ErrorKind::Other, format!("failed to send handshake: {}", e))
        })?;

        // 等待握手响应
        match tokio::time::timeout(std::time::Duration::from_secs(10), resp_rx.recv()).await {
            Ok(Some(resp)) if resp.cmd == 1 => {
                // HANDSHAKE_DONE
                debug!("Handshake successful for srcID: {}", src_id);
            }
            Ok(Some(resp)) if resp.cmd == 2 => {
                // HANDSHAKE_CONFLICT_SRC_ID
                return Err(io::Error::new(ErrorKind::Other, "srcID conflict"));
            }
            _ => {
                return Err(io::Error::new(ErrorKind::TimedOut, "handshake timeout"));
            }
        }

        Ok(Box::new(RogV2ClientRunStream::new(
            stream_manager.tx.clone(),
            resp_rx,
            src_id,
            host,
            port,
        )))
    }

    async fn udp_tunnel(
        &self,
        src_addr: String,
    ) -> io::Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        // 解析源地址以确定目标服务器
        // 这里假设src_addr格式为 "local_ip:local_port"，需要某种方式确定目标服务器
        // 实际实现中可能需要额外的参数或配置

        let server_addr = self.cfg.endpoint.as_ref().unwrap(); // 简化处理，使用配置的endpoint
        let stream_manager = self.get_or_create_stream(server_addr).await?;

        let src_id = Self::generate_src_id();

        // 创建用于接收UDP响应的channel
        let (resp_tx, mut resp_rx) = mpsc::channel::<StreamRes>(128);

        // 注册回调
        {
            let mut callbacks = stream_manager.callbacks.write().await;
            callbacks.insert(src_id.clone(), resp_tx);
        }

        // 发送UDP握手请求
        let handshake_req = StreamReq {
            auth: stream_manager.auth.clone(),
            payload: vec![],
            dst_addr: String::new(), // UDP不需要特定的目标地址
            dst_port: 0,
            src_addr: src_addr.clone(),
            src_port: 0,
            udp: true,
            cmd: 1, // HANDSHAKE_REQ
            src_id: src_id.clone(),
            addons: HashMap::new(),
        };

        stream_manager.tx.send(handshake_req).await.map_err(|e| {
            io::Error::new(
                ErrorKind::Other,
                format!("failed to send UDP handshake: {}", e),
            )
        })?;

        // 等待握手响应
        match tokio::time::timeout(std::time::Duration::from_secs(10), resp_rx.recv()).await {
            Ok(Some(resp)) if resp.cmd == 1 => {
                // HANDSHAKE_DONE
                debug!("UDP Handshake successful for srcID: {}", src_id);
            }
            Ok(Some(resp)) if resp.cmd == 2 => {
                // HANDSHAKE_CONFLICT_SRC_ID
                return Err(io::Error::new(ErrorKind::Other, "UDP srcID conflict"));
            }
            _ => {
                return Err(io::Error::new(ErrorKind::TimedOut, "UDP handshake timeout"));
            }
        }

        let reader = Box::new(RogV2UdpReader::new(resp_rx, src_id.clone()));
        let writer = Box::new(RogV2UdpWriter::new(
            stream_manager.tx.clone(),
            src_id,
            src_addr,
            stream_manager.auth.clone(),
        ));

        Ok(Some((reader, writer)))
    }
}

fn parse_address(addr: &str) -> io::Result<(String, u16)> {
    let (host, port) = addr.rsplit_once(':').ok_or(io::Error::new(
        ErrorKind::InvalidData,
        "invalid address format",
    ))?;

    let port: u16 = port
        .parse()
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid port"))?;

    if host.is_empty() {
        return Err(io::Error::new(ErrorKind::InvalidData, "empty host"));
    }

    Ok((host.to_string(), port))
}
