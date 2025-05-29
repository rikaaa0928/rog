use crate::def::{RunReadHalf, RunStream, RunWriteHalf, RunUdpReader, RunUdpWriter, UDPPacket};
use crate::proto::v2::pb::{StreamReq, StreamRes};
use crate::util::RunAddr;
use std::collections::HashMap;
use std::io::{Error, ErrorKind};
use tokio::sync::mpsc::{Receiver, Sender};
use log::debug;

// TCP Stream实现
pub struct RogV2ClientRunStream {
    tx: Sender<StreamReq>,
    rx: Receiver<StreamRes>,
    src_id: String,
    host: String,
    port: u16,
}

impl RogV2ClientRunStream {
    pub fn new(
        tx: Sender<StreamReq>,
        rx: Receiver<StreamRes>,
        src_id: String,
        host: String,
        port: u16,
    ) -> Self {
        Self { tx, rx, src_id, host, port }
    }
}

impl RunStream for RogV2ClientRunStream {
    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        let read_half = Box::new(RogV2ClientReadHalf {
            rx: self.rx,
            src_id: self.src_id.clone(),
        });
        
        let write_half = Box::new(RogV2ClientWriteHalf {
            tx: self.tx,
            src_id: self.src_id,
            host: self.host,
            port: self.port,
        });
        
        (read_half, write_half)
    }
}

pub struct RogV2ClientReadHalf {
    rx: Receiver<StreamRes>,
    src_id: String,
}

#[async_trait::async_trait]
impl RunReadHalf for RogV2ClientReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.rx.recv().await {
            Some(res) => {
                if res.cmd == 0 { // DATA
                    let payload = res.payload;
                    let len = payload.len().min(buf.len());
                    buf[..len].copy_from_slice(&payload[..len]);
                    Ok(len)
                } else {
                    // 处理其他命令
                    debug!("Received non-data command: {}", res.cmd);
                    Ok(0)
                }
            }
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
        Ok(None)
    }
}

pub struct RogV2ClientWriteHalf {
    tx: Sender<StreamReq>,
    src_id: String,
    host: String,
    port: u16,
}

#[async_trait::async_trait]
impl RunWriteHalf for RogV2ClientWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let req = StreamReq {
            auth: String::new(), // 已经在握手时认证
            payload: buf.to_vec(),
            dst_addr: self.host.clone(),
            dst_port: self.port as u32,
            src_addr: String::new(),
            src_port: 0,
            udp: false,
            cmd: 0, // DATA
            src_id: self.src_id.clone(),
            addons: HashMap::new(),
        };
        
        self.tx.send(req).await
            .map_err(|e| Error::new(ErrorKind::BrokenPipe, format!("send error: {}", e)))?;
        
        Ok(())
    }
}

// UDP实现
pub struct RogV2UdpReader {
    rx: Receiver<StreamRes>,
    src_id: String,
}

impl RogV2UdpReader {
    pub fn new(rx: Receiver<StreamRes>, src_id: String) -> Self {
        Self { rx, src_id }
    }
}

#[async_trait::async_trait]
impl RunUdpReader for RogV2UdpReader {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        match self.rx.recv().await {
            Some(res) if res.cmd == 0 && res.udp => {
                Ok(UDPPacket {
                    meta: crate::def::UDPMeta {
                        dst_addr: res.dst_addr,
                        dst_port: res.dst_port as u16,
                        src_addr: res.src_addr,
                        src_port: res.src_port as u16,
                    },
                    data: res.payload,
                })
            }
            Some(_) => Err(Error::new(ErrorKind::InvalidData, "invalid UDP response")),
            None => Err(Error::new(ErrorKind::UnexpectedEof, "stream closed")),
        }
    }
}

pub struct RogV2UdpWriter {
    tx: Sender<StreamReq>,
    src_id: String,
    src_addr: String,
    auth: String,
}

impl RogV2UdpWriter {
    pub fn new(tx: Sender<StreamReq>, src_id: String, src_addr: String, auth: String) -> Self {
        Self { tx, src_id, src_addr, auth }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for RogV2UdpWriter {
    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let (src_addr, src_port) = parse_address(&self.src_addr)?;
        
        let req = StreamReq {
            auth: self.auth.clone(),
            payload: packet.data,
            dst_addr: packet.meta.dst_addr,
            dst_port: packet.meta.dst_port as u32,
            src_addr,
            src_port: src_port as u32,
            udp: true,
            cmd: 0, // DATA
            src_id: self.src_id.clone(),
            addons: HashMap::new(),
        };
        
        self.tx.send(req).await
            .map_err(|e| Error::new(ErrorKind::BrokenPipe, format!("send error: {}", e)))?;
        
        Ok(())
    }
}

fn parse_address(addr: &str) -> std::io::Result<(String, u16)> {
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or(Error::new(ErrorKind::InvalidData, "invalid address format"))?;

    let port: u16 = port
        .parse()
        .map_err(|_| Error::new(ErrorKind::InvalidData, "invalid port"))?;

    Ok((host.to_string(), port))
}
