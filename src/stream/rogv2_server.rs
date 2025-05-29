use crate::def::{RunReadHalf, RunStream, RunWriteHalf, RunUdpReader, RunUdpWriter, UDPPacket, UDPMeta};
use crate::proto::v2::pb::{StreamReq, StreamRes};
use crate::util::RunAddr;
use std::collections::HashMap;
use std::io::{Error, ErrorKind};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{Receiver, Sender};
use tonic::Status;
use log::{debug, error};

// TCP Stream服务端实现
pub struct RogV2ServerStream {
    req_rx: Receiver<StreamReq>,
    res_tx: Sender<Result<StreamRes, Status>>,
    src_id: String,
    dst_addr: String,
    dst_port: u16,
}

impl RogV2ServerStream {
    pub fn new(
        req_rx: Receiver<StreamReq>,
        res_tx: Sender<Result<StreamRes, Status>>,
        src_id: String,
        dst_addr: String,
        dst_port: u16,
    ) -> Self {
        Self {
            req_rx,
            res_tx,
            src_id,
            dst_addr,
            dst_port,
        }
    }
    
    pub fn get_addr(&self) -> RunAddr {
        RunAddr {
            addr: self.dst_addr.clone(),
            port: self.dst_port,
            udp: false,
            cache: None,
        }
    }
}

impl RunStream for RogV2ServerStream {
    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        let read_half = Box::new(RogV2ServerReadHalf {
            req_rx: self.req_rx,
            src_id: self.src_id.clone(),
            dst_addr: self.dst_addr.clone(),
            dst_port: self.dst_port,
        });
        
        let write_half = Box::new(RogV2ServerWriteHalf {
            res_tx: self.res_tx,
            src_id: self.src_id,
        });
        
        (read_half, write_half)
    }
}

pub struct RogV2ServerReadHalf {
    req_rx: Receiver<StreamReq>,
    src_id: String,
    dst_addr: String,
    dst_port: u16,
}

#[async_trait::async_trait]
impl RunReadHalf for RogV2ServerReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.req_rx.recv().await {
            Some(req) if req.cmd == 0 && !req.udp => {
                let payload = req.payload;
                let len = payload.len().min(buf.len());
                buf[..len].copy_from_slice(&payload[..len]);
                Ok(len)
            }
            Some(_) => Ok(0),
            None => Err(Error::new(ErrorKind::UnexpectedEof, "stream closed")),
        }
    }

    async fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut total = 0;
        while total < buf.len() {
            match self.read(&mut buf[total..]).await? {
                0 => return Err(Error::new(ErrorKind::UnexpectedEof, "unexpected EOF")),
                n => total += n,
            }
        }
        Ok(total)
    }

    async fn handshake(&self) -> std::io::Result<Option<(RunAddr, String)>> {
        // 返回目标地址信息
        let addr = RunAddr {
            addr: self.dst_addr.clone(),
            port: self.dst_port,
            udp: false,
            cache: None,
        };
        Ok(Some((addr, String::new())))
    }
}

pub struct RogV2ServerWriteHalf {
    res_tx: Sender<Result<StreamRes, Status>>,
    src_id: String,
}

#[async_trait::async_trait]
impl RunWriteHalf for RogV2ServerWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let res = StreamRes {
            payload: buf.to_vec(),
            dst_addr: String::new(),
            dst_port: 0,
            src_addr: String::new(),
            src_port: 0,
            udp: false,
            cmd: 0, // DATA
            src_id: self.src_id.clone(),
            addons: HashMap::new(),
        };
        
        self.res_tx.send(Ok(res)).await
            .map_err(|e| Error::new(ErrorKind::BrokenPipe, format!("send error: {}", e)))?;
        
        Ok(())
    }
}

// UDP Socket服务端实现
pub struct RogV2UdpSocket {
    req_rx: Receiver<StreamReq>,
    res_tx: Sender<Result<StreamRes, Status>>,
    src_id: String,
}

impl RogV2UdpSocket {
    pub fn new(
        req_rx: Receiver<StreamReq>,
        res_tx: Sender<Result<StreamRes, Status>>,
        src_id: String,
    ) -> Self {
        Self {
            req_rx,
            res_tx,
            src_id,
        }
    }

    pub async fn run(mut self) -> std::io::Result<()> {
        // 绑定UDP socket
        let socket = std::sync::Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
        debug!("UDP socket bound to {}", socket.local_addr()?);
        
        // 处理UDP数据包
        let socket_recv = std::sync::Arc::clone(&socket);
        let res_tx = self.res_tx.clone();
        let src_id = self.src_id.clone();
        
        // 启动接收任务
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                match socket_recv.recv_from(&mut buf).await {
                    Ok((len, addr)) => {
                        let res = StreamRes {
                            payload: buf[..len].to_vec(),
                            dst_addr: String::new(), // 原始目标地址
                            dst_port: 0,
                            src_addr: addr.ip().to_string(),
                            src_port: addr.port() as u32,
                            udp: true,
                            cmd: 0, // DATA
                            src_id: src_id.clone(),
                            addons: HashMap::new(),
                        };
                        
                        if let Err(e) = res_tx.send(Ok(res)).await {
                            error!("Failed to send UDP response: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("UDP receive error: {}", e);
                        break;
                    }
                }
            }
        });
        
        // 处理发送请求
        while let Some(req) = self.req_rx.recv().await {
            if req.cmd == 0 && req.udp {
                let addr: SocketAddr = format!("{}:{}", req.dst_addr, req.dst_port)
                    .parse()
                    .map_err(|e| Error::new(ErrorKind::InvalidInput, format!("invalid address: {}", e)))?;
                
                if let Err(e) = socket.send_to(&req.payload, addr).await {
                    error!("UDP send error: {}", e);
                }
            }
        }
        
        Ok(())
    }
}

// UDP Reader/Writer 包装器，用于与现有系统集成
pub struct RogV2UdpReader {
    req_rx: Receiver<StreamReq>,
    src_id: String,
}

impl RogV2UdpReader {
    pub fn new(req_rx: Receiver<StreamReq>, src_id: String) -> Self {
        Self { req_rx, src_id }
    }
}

#[async_trait::async_trait]
impl RunUdpReader for RogV2UdpReader {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        match self.req_rx.recv().await {
            Some(req) if req.cmd == 0 && req.udp => {
                Ok(UDPPacket {
                    meta: crate::def::UDPMeta {
                        dst_addr: req.dst_addr,
                        dst_port: req.dst_port as u16,
                        src_addr: req.src_addr,
                        src_port: req.src_port as u16,
                    },
                    data: req.payload,
                })
            }
            Some(_) => Err(Error::new(ErrorKind::InvalidData, "invalid UDP request")),
            None => Err(Error::new(ErrorKind::UnexpectedEof, "stream closed")),
        }
    }
}

pub struct RogV2UdpWriter {
    res_tx: Sender<StreamRes>,
    src_id: String,
}

impl RogV2UdpWriter {
    pub fn new(res_tx: Sender<StreamRes>, src_id: String) -> Self {
        Self { res_tx, src_id }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for RogV2UdpWriter {
    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let res = StreamRes {
            payload: packet.data,
            dst_addr: packet.meta.dst_addr,
            dst_port: packet.meta.dst_port as u32,
            src_addr: packet.meta.src_addr,
            src_port: packet.meta.src_port as u32,
            udp: true,
            cmd: 0, // DATA
            src_id: self.src_id.clone(),
            addons: HashMap::new(),
        };
        
        self.res_tx.send(res).await
            .map_err(|e| Error::new(ErrorKind::BrokenPipe, format!("send error: {}", e)))?;
        
        Ok(())
    }
}
