use crate::def::ReadWrite;
use crate::proto::v1::pb::{StreamReq, StreamRes};
use crate::util::RunAddr;
use futures::{Stream, StreamExt};
use std::io::ErrorKind;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tonic::{Status, Streaming};

pub struct GrpcServerRunStream {
    reader: Arc<Mutex<Streaming<StreamReq>>>,
    writer: Sender<std::result::Result<StreamRes, Status>>,
    handshake_done: bool,
    buffer: Vec<u8>,
    read_pos: usize,
}

impl AsyncRead for GrpcServerRunStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.read_pos >= self.buffer.len() {
            let next_item = {
                let mut reader_lock = self.reader.try_lock().unwrap();
                let stream = Pin::new(&mut *reader_lock);
                stream.poll_next(cx)
            };

            match next_item {
                Poll::Ready(Some(Ok(res))) => {
                    self.buffer = res.payload.unwrap_or_default();
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

impl AsyncWrite for GrpcServerRunStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut res = StreamRes::default();
        res.payload = buf.to_vec();
        match self.writer.try_send(Ok(res)) {
            Ok(_) => Poll::Ready(Ok(buf.len())),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Poll::Pending,
            Err(e) => Poll::Ready(Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string()))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl GrpcServerRunStream {
    pub fn new(
        reader: Arc<Mutex<Streaming<StreamReq>>>,
        writer: Sender<std::result::Result<StreamRes, Status>>,
    ) -> Self {
        Self {
            reader,
            writer,
            handshake_done: false,
            buffer: Vec::new(),
            read_pos: 0,
        }
    }

    pub async fn handshake(&mut self) -> std::io::Result<Option<(RunAddr, String)>> {
        if self.handshake_done {
            return Ok(None);
        }
        let auth = self.reader.lock().await.next().await;
        self.handshake_done = true;
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