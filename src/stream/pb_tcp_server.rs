use crate::def::{RunReadHalf, RunStream, RunWriteHalf, StreamInfo};
use crate::proto::v1::pb::{StreamReq, StreamRes};
use crate::util::RunAddr;
use crate::util::crypto::decrypt_field;
use crate::util::tcp_frame::{read_msg, write_frame};
use std::any::Any;
use std::sync::Arc;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;

pub struct PbTcpServerReadHalf {
    reader: Arc<Mutex<OwnedReadHalf>>,
    cache: Vec<u8>,
    cache_pos: usize,
}

pub struct PbTcpServerWriteHalf {
    writer: Arc<Mutex<OwnedWriteHalf>>,
}

pub struct PbTcpServerRunStream {
    reader: Arc<Mutex<OwnedReadHalf>>,
    writer: Arc<Mutex<OwnedWriteHalf>>,
    cache: Vec<u8>,
    cache_pos: usize,
    info: StreamInfo,
}

impl PbTcpServerRunStream {
    pub fn new(reader: OwnedReadHalf, writer: OwnedWriteHalf) -> Self {
        Self {
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            cache: Vec::new(),
            cache_pos: 0,
            info: StreamInfo::default(),
        }
    }

    pub async fn handshake(&mut self, pw: &str) -> std::io::Result<Option<(RunAddr, String)>> {
        let mut r = self.reader.lock().await;
        let req: StreamReq = read_msg(&mut *r).await?;
        let auth = decrypt_field(&req.auth, pw)?;
        let dst_addr = match req.dst_addr {
            Some(ref enc) => decrypt_field(enc, pw)?,
            None => return Err(std::io::Error::other("missing dst_addr")),
        };
        let dst_port = req.dst_port.ok_or_else(|| std::io::Error::other("missing dst_port"))?;
        let ra = RunAddr {
            addr: dst_addr,
            port: dst_port as u16,
            udp: false,
        };
        Ok(Some((ra, auth)))
    }
}

async fn read_payload(
    reader: &Arc<Mutex<OwnedReadHalf>>,
    cache: &mut Vec<u8>,
    cache_pos: &mut usize,
    buf: &mut [u8],
) -> std::io::Result<usize> {
    if *cache_pos < cache.len() {
        let available = cache.len() - *cache_pos;
        let to_copy = available.min(buf.len());
        buf[..to_copy].copy_from_slice(&cache[*cache_pos..*cache_pos + to_copy]);
        *cache_pos += to_copy;
        if *cache_pos >= cache.len() {
            cache.clear();
            *cache_pos = 0;
        }
        return Ok(to_copy);
    }

    cache.clear();
    *cache_pos = 0;

    let mut r = reader.lock().await;
    let msg: StreamReq = read_msg(&mut *r).await?;
    let payload = msg.payload.unwrap_or_default();
    if payload.is_empty() {
        return Ok(0);
    }
    let to_copy = buf.len().min(payload.len());
    buf[..to_copy].copy_from_slice(&payload[..to_copy]);
    if payload.len() > to_copy {
        cache.extend_from_slice(&payload[to_copy..]);
    }
    Ok(to_copy)
}

#[async_trait::async_trait]
impl RunReadHalf for PbTcpServerReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        read_payload(&self.reader, &mut self.cache, &mut self.cache_pos, buf).await
    }
}

#[async_trait::async_trait]
impl RunWriteHalf for PbTcpServerWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let res = StreamRes {
            payload: buf.to_vec(),
        };
        let mut w = self.writer.lock().await;
        write_frame(&mut *w, &res).await
    }
}

#[async_trait::async_trait]
impl RunStream for PbTcpServerRunStream {
    fn get_info(&self) -> &StreamInfo {
        &self.info
    }

    fn set_info(&mut self, f: &mut dyn FnMut(&mut StreamInfo)) {
        f(&mut self.info)
    }

    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        (
            Box::new(PbTcpServerReadHalf {
                reader: Arc::clone(&self.reader),
                cache: Vec::new(),
                cache_pos: 0,
            }),
            Box::new(PbTcpServerWriteHalf {
                writer: Arc::clone(&self.writer),
            }),
        )
    }

    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        read_payload(&self.reader, &mut self.cache, &mut self.cache_pos, buf).await
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let res = StreamRes {
            payload: buf.to_vec(),
        };
        let mut w = self.writer.lock().await;
        write_frame(&mut *w, &res).await
    }
}
