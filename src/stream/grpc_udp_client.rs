use crate::def::{RunUdpStream, UDPMeta, UDPPacket};
use crate::stream::grpc_client::pb::{UdpReq, UdpRes};
use futures::StreamExt;
use log::debug;
use std::io::Error;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tonic::Streaming;

pub struct GrpcUdpClientRunStream {
    reader: Arc<Mutex<Streaming<UdpRes>>>,
    writer: Sender<UdpReq>,
    src_addr: String,
    auth: String,
}

impl GrpcUdpClientRunStream {
    pub fn new(
        reader: Arc<Mutex<Streaming<UdpRes>>>,
        writer: Sender<UdpReq>,
        src_addr: String,
        auth: String,
    ) -> Self {
        Self {
            reader,
            writer,
            src_addr,
            auth,
        }
    }
}

#[async_trait::async_trait]
impl RunUdpStream for GrpcUdpClientRunStream {
    async fn read(&self) -> std::io::Result<UDPPacket> {
        match self.reader.lock().await.next().await {
            Some(Err(e)) => Err(Error::new(std::io::ErrorKind::BrokenPipe, e.to_string())),
            Some(Ok(res)) => {
                let udp = res.try_into().unwrap();
                debug!("grpc read UDP packet {:?}", &udp);
                Ok(udp)
            }
            None => Err(Error::new(std::io::ErrorKind::Other, "stream closed")),
        }
    }

    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let req = UdpReq {
            auth: self.auth.clone(),
            payload: Some(packet.data),
            dst_addr: Some(packet.meta.dst_addr.clone()),
            dst_port: Some(packet.meta.dst_port.clone() as u32),
            src_addr: Some(packet.meta.src_addr.clone()),
            src_port: Some(packet.meta.src_port.clone() as u32),
        };
        debug!("grpc Sending UDP request {:?}", &req);
        match self.writer.send(req).await {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::new(std::io::ErrorKind::BrokenPipe, e.to_string())),
        }
    }
}
