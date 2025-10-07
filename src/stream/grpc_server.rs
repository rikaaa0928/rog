use crate::def::{RunReadHalf, RunStream, RunWriteHalf, StreamInfo};
use crate::proto::v1::pb::{StreamReq, StreamRes};
use crate::util::RunAddr;
use futures::StreamExt;
use std::any::Any;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tonic::{Status, Streaming};

// pub mod pb {
//     tonic::include_proto!("moe.rikaaa0928.rog");
// }

pub struct GrpcServerReadHalf {
    reader: Arc<Mutex<Streaming<StreamReq>>>,
    cache: Vec<u8>,
    cache_pos: usize,
}

pub struct GrpcServerWriteHalf {
    writer: Sender<Result<StreamRes, Status>>,
}

pub struct GrpcServerRunStream {
    reader: Arc<Mutex<Streaming<StreamReq>>>,
    writer: Sender<Result<StreamRes, Status>>,
    cache: Vec<u8>,
    cache_pos: usize,
    info: StreamInfo,
}

#[async_trait::async_trait]
impl RunReadHalf for GrpcServerReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cache_pos < self.cache.len() {
            let available = self.cache.len() - self.cache_pos;
            let to_copy = available.min(buf.len());
            buf[..to_copy].copy_from_slice(&self.cache[self.cache_pos..self.cache_pos + to_copy]);
            self.cache_pos += to_copy;
            if self.cache_pos >= self.cache.len() {
                self.cache.clear();
                self.cache_pos = 0;
            }
            return Ok(to_copy);
        }

        self.cache.clear();
        self.cache_pos = 0;

        let res = self.reader.lock().await.next().await;
        if res.is_none() {
            return Err(std::io::Error::new(ErrorKind::Other, "no more data"));
        }
        let res = res.unwrap();
        match res {
            Ok(data) => {
                let payload = match data.payload {
                    Some(p) => p,
                    None => Vec::new(),
                };
                if payload.is_empty() {
                    return Ok(0);
                }
                let to_copy = buf.len().min(payload.len());
                buf[..to_copy].copy_from_slice(&payload[..to_copy]);
                if payload.len() > to_copy {
                    self.cache.extend_from_slice(&payload[to_copy..]);
                }
                Ok(to_copy)
            }
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }

}

#[async_trait::async_trait]
impl RunWriteHalf for GrpcServerWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let mut res = StreamRes::default();
        res.payload = buf.to_vec();
        match self.writer.send(Ok(res)).await {
            Ok(_) => Ok(()),
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}

impl GrpcServerRunStream {
    pub fn new(
        reader: Arc<Mutex<Streaming<StreamReq>>>,
        writer: Sender<Result<StreamRes, Status>>,
    ) -> Self {
        Self {
            reader,
            writer,
            cache: Vec::new(),
            cache_pos: 0,
            info: StreamInfo::default(),
        }
    }

    pub async fn handshake(&mut self) -> std::io::Result<Option<(RunAddr, String)>> {
        let auth = self.reader.lock().await.next().await;
        match auth {
            Some(Ok(a)) => {
                let pw = a.auth;
                let ra = RunAddr {
                    addr: a.dst_addr.unwrap(),
                    port: a.dst_port.unwrap() as u16,
                    udp: false,
                };
                Ok(Some((ra, pw)))
            }
            Some(Err(err)) => Err(std::io::Error::new(ErrorKind::Interrupted, err.to_string())),
            None => Err(std::io::Error::new(ErrorKind::Interrupted, "interrupted")),
        }
    }
}

#[async_trait::async_trait]
impl RunStream for GrpcServerRunStream {
    fn get_info(&self) -> &StreamInfo {
        &self.info
    }

    fn set_info(&mut self, f: &mut dyn FnMut(&mut StreamInfo)) {
        f(&mut self.info)
    }

    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        (
            Box::new(GrpcServerReadHalf {
                reader: Arc::clone(&self.reader),
                cache: Vec::new(),
                cache_pos: 0,
            }),
            Box::new(GrpcServerWriteHalf {
                writer: self.writer,
            }),
        )
    }

    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cache_pos < self.cache.len() {
            let available = self.cache.len() - self.cache_pos;
            let to_copy = available.min(buf.len());
            buf[..to_copy].copy_from_slice(&self.cache[self.cache_pos..self.cache_pos + to_copy]);
            self.cache_pos += to_copy;
            if self.cache_pos >= self.cache.len() {
                self.cache.clear();
                self.cache_pos = 0;
            }
            return Ok(to_copy);
        }

        self.cache.clear();
        self.cache_pos = 0;

        let res = self.reader.lock().await.next().await;
        if res.is_none() {
            return Err(std::io::Error::new(ErrorKind::Other, "no more data"));
        }
        let res = res.unwrap();
        match res {
            Ok(data) => {
                let payload = match data.payload {
                    Some(p) => p,
                    None => Vec::new(),
                };
                if payload.is_empty() {
                    return Ok(0);
                }
                let to_copy = buf.len().min(payload.len());
                buf[..to_copy].copy_from_slice(&payload[..to_copy]);
                if payload.len() > to_copy {
                    self.cache.extend_from_slice(&payload[to_copy..]);
                }
                Ok(to_copy)
            }
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let mut res = StreamRes::default();
        res.payload = buf.to_vec();
        match self.writer.send(Ok(res)).await {
            Ok(_) => Ok(()),
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}
