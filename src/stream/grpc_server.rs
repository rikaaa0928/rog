use std::error::Error;
use std::io::ErrorKind;
use std::sync::Arc;
use futures::StreamExt;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tonic::{Status, Streaming};
use crate::def::{RunReadHalf, RunStream, RunWriteHalf};
use crate::stream::grpc_client::pb::{StreamReq, StreamRes};
use crate::util::RunAddr;

pub mod pb {
    tonic::include_proto!("moe.rikaaa0928.rog");
}

pub struct GrpcServerReadHalf {
    reader: Arc<Mutex<Streaming<StreamReq>>>,
}

pub struct GrpcServerWriteHalf {
    writer: Sender<Result<StreamRes, Status>>,
}

pub struct GrpcServerRunStream {
    reader: Arc<Mutex<Streaming<StreamReq>>>,
    writer: Sender<Result<StreamRes, Status>>,
}

#[async_trait::async_trait]
impl RunReadHalf for GrpcServerReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let res = self.reader.lock().await.next().await;
        if res.is_none() {
            return Err(std::io::Error::new(ErrorKind::Other, "no more data"));
        }
        let res = res.unwrap();
        match res {
            Ok(data) => {
                let n = data.payload.as_ref().unwrap().len();
                buf[..n].copy_from_slice(data.payload.as_ref().unwrap());
                Ok(n)
            }
            Err(e) => {
                Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string()))
            }
        }
    }

    async fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }

    async fn handshake(&self) -> std::io::Result<Option<(RunAddr, String)>> {
        let auth = self.reader.lock().await.next().await.unwrap();
        match auth {
            Ok(a) => {
                let pw = a.auth;
                let ra = RunAddr {
                    addr: a.dst_addr.unwrap(),
                    port: a.dst_port.unwrap() as u16,
                    // a_type: 0,
                    udp: false,
                };
                Ok(Some((ra, pw)))
            }
            Err(err) => {
                Err(std::io::Error::new(ErrorKind::Interrupted, err.to_string()))
            }
        }
    }
}

#[async_trait::async_trait]
impl RunWriteHalf for GrpcServerWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let mut res = StreamRes::default();
        res.payload = buf.to_vec();
        match self.writer.send(Ok(res)).await {
            Ok(_) => {
                Ok(())
            }
            Err(e) => {
                Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string()))
            }
        }
    }
}

impl GrpcServerRunStream {
    pub fn new(reader: Arc<Mutex<Streaming<StreamReq>>>, writer: Sender<Result<StreamRes, Status>>) -> Self {
        Self {
            reader,
            writer,
        }
    }
}

#[async_trait::async_trait]
impl RunStream for GrpcServerRunStream {
    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        (Box::new(GrpcServerReadHalf { reader: Arc::clone(&self.reader) }), Box::new(GrpcServerWriteHalf {
            writer: self.writer,
        }))
    }
}