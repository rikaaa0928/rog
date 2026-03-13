use crate::def::{RunReadHalf, RunStream, RunWriteHalf, StreamInfo};
use crate::proto::v1::pb::{RevStreamReq, RevStreamRes};
use crate::util::RunAddr;
use futures::StreamExt;
use std::any::Any;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;
use tonic::{Status, Streaming};

// pub mod pb {
//     tonic::include_proto!("moe.rikaaa0928.rog");
// }

pub struct RevGrpcServerReadHalf {
    reader: Arc<Mutex<Streaming<RevStreamReq>>>,
    cache: Vec<u8>,
    cache_pos: usize,
}

pub struct RevGrpcServerWriteHalf {
    writer: Sender<Result<RevStreamRes, Status>>,
}

pub struct RevGrpcServerRunStream {
    reader: Arc<Mutex<Streaming<RevStreamReq>>>,
    writer: Sender<Result<RevStreamRes, Status>>,
    cache: Vec<u8>,
    cache_pos: usize,
    info: StreamInfo,
    dst_addr: String,
    dst_port: u16,
}

#[async_trait::async_trait]
impl RunReadHalf for RevGrpcServerReadHalf {
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
            return Err(std::io::Error::other("no more data"));
        }
        let res = res.unwrap();
        match res {
            Ok(data) => {
                let payload = data.payload.unwrap();
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
impl RunWriteHalf for RevGrpcServerWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let res = RevStreamRes {
            payload: buf.to_vec(),
        };
        match self.writer.send(Ok(res)).await {
            Ok(_) => Ok(()),
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}

impl RevGrpcServerRunStream {
    pub fn new(
        reader: Arc<Mutex<Streaming<RevStreamReq>>>,
        writer: Sender<Result<RevStreamRes, Status>>,
        dst_addr: String,
        dst_port: u16,
    ) -> Self {
        Self {
            reader,
            writer,
            cache: Vec::new(),
            cache_pos: 0,
            info: StreamInfo::default(),
            dst_port,
            dst_addr,
        }
    }

    // pub async fn handshake(&mut self) -> std::io::Result<Option<(RunAddr, String)>> {
    //     let ra = RunAddr {
    //         addr: self.dst_addr.clone(),
    //         port: self.dst_port,
    //         udp: false,
    //     };
    //     Ok(Some((ra, "".to_string())))
    // }
}

#[async_trait::async_trait]
impl RunStream for RevGrpcServerRunStream {
    fn get_info(&self) -> &StreamInfo {
        &self.info
    }

    fn set_info(&mut self, f: &mut dyn FnMut(&mut StreamInfo)) {
        f(&mut self.info)
    }

    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        (
            Box::new(RevGrpcServerReadHalf {
                reader: Arc::clone(&self.reader),
                cache: Vec::new(),
                cache_pos: 0,
            }),
            Box::new(RevGrpcServerWriteHalf {
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
            return Err(std::io::Error::other("no more data"));
        }
        let res = res.unwrap();
        match res {
            Ok(data) => {
                let payload = data.payload.unwrap();
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

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let res = RevStreamRes {
            payload: buf.to_vec(),
        };
        match self.writer.send(Ok(res)).await {
            Ok(_) => Ok(()),
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}
