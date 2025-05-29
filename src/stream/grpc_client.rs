use crate::def::{RunReadHalf, RunStream, RunWriteHalf};
use crate::util::RunAddr;
use futures::StreamExt;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tonic::Streaming;
use crate::proto::v1::pb::{StreamReq, StreamRes};

pub struct GrpcClientReadHalf {
    reader: Arc<Mutex<Streaming<StreamRes>>>,
}

pub struct GrpcClientWriteHalf {
    writer: Sender<StreamReq>,
}

pub struct GrpcClientRunStream {
    reader: Arc<Mutex<Streaming<StreamRes>>>,
    writer: Sender<StreamReq>,
}

#[async_trait::async_trait]
impl RunReadHalf for GrpcClientReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let res = self.reader.lock().await.next().await;
        if res.is_none() {
            return Err(std::io::Error::new(ErrorKind::Other, "no more data"));
        }
        let res = res.unwrap();
        match res {
            Ok(data) => {
                let n = data.payload.len();
                buf[..n].copy_from_slice(&data.payload);
                Ok(n)
            }
            Err(e) => {
                Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string()))
            }
        }
    }

    async fn read_exact(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }

    async fn handshake(&self) -> std::io::Result<Option<(RunAddr, String)>> {
        Ok(None)
    }
}

#[async_trait::async_trait]
impl RunWriteHalf for GrpcClientWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let mut req = StreamReq::default();
        req.payload = Some(buf.to_vec());
        match self.writer.send(req).await {
            Ok(_) => {
                Ok(())
            }
            Err(e) => {
                Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string()))
            }
        }
    }
}

impl GrpcClientRunStream {
    pub fn new(reader: Arc<Mutex<Streaming<StreamRes>>>, writer: Sender<StreamReq>) -> Self {
        Self {
            reader,
            writer,
        }
    }
}

#[async_trait::async_trait]
impl RunStream for GrpcClientRunStream {
    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        (Box::new(GrpcClientReadHalf { reader: Arc::clone(&self.reader) }), Box::new(GrpcClientWriteHalf {
            writer: self.writer,
        }))
    }
}