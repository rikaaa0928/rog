use crate::def::{RunUdpReader, RunUdpWriter, UDPPacket};
use crate::proto::v1::pb::{UdpReq, UdpRes};
use futures::StreamExt;
use log::debug;
use std::io::Error;
use tokio::sync::mpsc::Sender;
use tonic::Streaming;

pub struct GrpcUdpClientRunWriter {
    writer: Sender<UdpReq>,
    src_addr: String,
    auth: String,
}

pub struct GrpcUdpClientRunReader {
    reader: Option<Streaming<UdpRes>>,
    init: Option<tokio::task::JoinHandle<std::io::Result<Streaming<UdpRes>>>>,
    src_addr: String,
    auth: String,
}

impl GrpcUdpClientRunReader {
    pub fn new(
        init: tokio::task::JoinHandle<std::io::Result<Streaming<UdpRes>>>,
        src_addr: String,
        auth: String,
    ) -> Self {
        Self {
            reader: None,
            init: Some(init),
            src_addr,
            auth,
        }
    }
}

impl Drop for GrpcUdpClientRunReader {
    fn drop(&mut self) {
        if let Some(handle) = self.init.take() {
            handle.abort();
        }
    }
}

impl GrpcUdpClientRunWriter {
    pub fn new(writer: Sender<UdpReq>, src_addr: String, auth: String) -> Self {
        Self {
            writer,
            src_addr,
            auth,
        }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for GrpcUdpClientRunWriter {
    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let req = UdpReq::from_packet(packet, self.auth.clone());
        debug!("grpc Sending UDP request {:?}", &req);
        match self.writer.send(req).await {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::new(std::io::ErrorKind::BrokenPipe, e.to_string())),
        }
    }
}

#[async_trait::async_trait]
impl RunUdpReader for GrpcUdpClientRunReader {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        if self.reader.is_none() {
            let handle = self
                .init
                .take()
                .ok_or_else(|| Error::other("udp stream init missing"))?;
            let stream = handle
                .await
                .map_err(|e| Error::other(format!("udp stream join error: {e}")))??;
            self.reader = Some(stream);
        }
        let reader = self.reader.as_mut().unwrap();
        match reader.next().await {
            Some(Err(e)) => Err(Error::new(std::io::ErrorKind::BrokenPipe, e.to_string())),
            Some(Ok(res)) => {
                let udp = res.try_into().unwrap();
                debug!("grpc read UDP packet {:?}", &udp);
                Ok(udp)
            }
            None => Err(Error::other("stream closed")),
        }
    }
}
