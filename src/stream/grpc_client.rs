use crate::def::ReadWrite;
use crate::proto::v1::pb::{StreamReq, StreamRes};
use futures::Stream;
use std::io::{ErrorKind, Result};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
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
    read_half: GrpcClientReadHalf,
    write_half: GrpcClientWriteHalf,
}

impl AsyncRead for GrpcClientReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<()>> {
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
                Poll::Ready(None) => return Poll::Ready(Ok(())), // EOF
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

impl AsyncWrite for GrpcClientWriteHalf {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        let mut req = StreamReq::default();
        req.payload = Some(buf.to_vec());
        match self.writer.try_send(req) {
            Ok(_) => Poll::Ready(Ok(buf.len())),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Poll::Pending,
            Err(e) => Poll::Ready(Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string()))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl GrpcClientRunStream {
    pub fn new(reader: Arc<Mutex<Streaming<StreamRes>>>, writer: Sender<StreamReq>) -> Self {
        Self {
            read_half: GrpcClientReadHalf {
                reader,
                buffer: Vec::new(),
                read_pos: 0,
            },
            write_half: GrpcClientWriteHalf { writer },
        }
    }
}

impl AsyncRead for GrpcClientRunStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<()>> {
        Pin::new(&mut self.read_half).poll_read(cx, buf)
    }
}

impl AsyncWrite for GrpcClientRunStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.write_half).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.write_half).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.write_half).poll_shutdown(cx)
    }
}