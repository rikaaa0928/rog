use crate::def::{RunReadHalf, RunStream, RunWriteHalf, StreamInfo};
use crate::proto::v1::pb::{StreamReq, StreamRes};
use futures::StreamExt;
use std::any::Any;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tonic::Streaming;

pub struct GrpcClientReadHalf {
    reader: Arc<Mutex<Streaming<StreamRes>>>,
    cache: Vec<u8>,
    cache_pos: usize,
}

pub struct GrpcClientWriteHalf {
    writer: Sender<StreamReq>,
}

pub struct GrpcClientRunStream {
    reader: Arc<Mutex<Streaming<StreamRes>>>,
    writer: Sender<StreamReq>,
    cache: Vec<u8>,
    cache_pos: usize,
    info: StreamInfo,
}

#[async_trait::async_trait]
impl RunReadHalf for GrpcClientReadHalf {
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
                if data.payload.is_empty() {
                    return Ok(0);
                }
                let to_copy = buf.len().min(data.payload.len());
                buf[..to_copy].copy_from_slice(&data.payload[..to_copy]);
                if data.payload.len() > to_copy {
                    self.cache.extend_from_slice(&data.payload[to_copy..]);
                }
                Ok(to_copy)
            }
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}

#[async_trait::async_trait]
impl RunWriteHalf for GrpcClientWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let req = StreamReq { payload: Some(buf.to_vec()), ..Default::default() };
        match self.writer.send(req).await {
            Ok(_) => Ok(()),
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}

impl GrpcClientRunStream {
    pub fn new(reader: Arc<Mutex<Streaming<StreamRes>>>, writer: Sender<StreamReq>) -> Self {
        Self {
            reader,
            writer,
            cache: Vec::new(),
            cache_pos: 0,
            info: StreamInfo::default(),
        }
    }
}

#[async_trait::async_trait]
impl RunStream for GrpcClientRunStream {
    fn get_info(&self) -> &StreamInfo {
        &self.info
    }

    fn set_info(&mut self, f: &mut dyn FnMut(&mut StreamInfo)) {
        f(&mut self.info)
    }
    fn into_tcp_stream(self: Box<Self>) -> std::result::Result<tokio::net::TcpStream, Box<dyn RunStream>> {
        Err(self)
    }

    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        (
            Box::new(GrpcClientReadHalf {
                reader: Arc::clone(&self.reader),
                cache: Vec::new(),
                cache_pos: 0,
            }),
            Box::new(GrpcClientWriteHalf {
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
                if data.payload.is_empty() {
                    return Ok(0);
                }
                let to_copy = buf.len().min(data.payload.len());
                buf[..to_copy].copy_from_slice(&data.payload[..to_copy]);
                if data.payload.len() > to_copy {
                    self.cache.extend_from_slice(&data.payload[to_copy..]);
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
        let req = StreamReq { payload: Some(buf.to_vec()), ..Default::default() };
        match self.writer.send(req).await {
            Ok(_) => Ok(()),
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}
