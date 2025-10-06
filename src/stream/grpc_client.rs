use crate::def::{RunReadHalf, RunStream, RunWriteHalf};
use crate::proto::v1::pb::{StreamReq, StreamRes};
use std::any::Any;
use std::io::{Error, ErrorKind};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tonic::codegen::tokio_stream::{Stream, StreamExt};
use tonic::Streaming;

pub struct GrpcClientReadHalf {
    reader: Arc<Mutex<Streaming<StreamRes>>>,
    buffer: Vec<u8>,
    read_pos: usize,
}

pub struct GrpcClientWriteHalf {
    writer: Sender<StreamReq>,
}

pub struct GrpcClientRunStream {
    reader: Arc<Mutex<Streaming<StreamRes>>>,
    writer: Sender<StreamReq>,
    cache: Vec<u8>,
    cache_pos: usize,
}

#[async_trait::async_trait]
impl AsyncRead for GrpcClientReadHalf {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        if self.read_pos >= self.buffer.len() {
            let next_item = {
                let mut reader_lock = self.reader.try_lock().unwrap();
                let stream = Pin::new(&mut *reader_lock);
                stream.poll_next(cx)
            };

            match next_item {
                Poll::Ready(Some(Ok(res))) => {
                    self.buffer = res.payload;
                    self.read_pos = 0;
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Err(std::io::Error::new(
                        ErrorKind::Interrupted,
                        e.to_string(),
                    )))
                }
                Poll::Ready(None) => return return Poll::Pending,
                Poll::Pending => return Poll::Pending,
            }
        }

        let remaining = self.buffer.len() - self.read_pos;
        let amt = std::cmp::min(remaining, buf.remaining());
        buf.put_slice(&self.buffer[self.read_pos..self.read_pos + amt]);
        self.read_pos += amt;

        Poll::Ready(Ok(()))
    }
}

#[async_trait::async_trait]
impl AsyncWrite for GrpcClientWriteHalf {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, Error>> {
        let mut req = StreamReq::default();
        req.payload = Some(buf.to_vec());
        match self.writer.try_send(req) {
            Ok(_) => Poll::Ready(Ok(buf.len())),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Poll::Pending,
            Err(e) => Poll::Ready(Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string()))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }
}

impl GrpcClientRunStream {
    pub fn new(reader: Arc<Mutex<Streaming<StreamRes>>>, writer: Sender<StreamReq>) -> Self {
        Self {
            reader,
            writer,
            cache: Vec::new(),
            cache_pos: 0,
        }
    }
}

#[async_trait::async_trait]
impl RunStream for GrpcClientRunStream {
    fn split(self: Box<Self>) -> (Box<RunReadHalf>, Box<RunWriteHalf>) {
        (
            Box::new(GrpcClientReadHalf {
                reader: Arc::clone(&self.reader),
                buffer: Vec::new(),
                read_pos: 0,
            }),
            Box::new(GrpcClientWriteHalf {
                writer: self.writer,
            }),
        )
    }

    async fn stream_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
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

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    async fn stream_write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let mut req = StreamReq::default();
        req.payload = Some(buf.to_vec());
        match self.writer.send(req).await {
            Ok(_) => Ok(()),
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}
