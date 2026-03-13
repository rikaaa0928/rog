use crate::def::{RunUdpReader, RunUdpWriter, UDPPacket};
use crate::proto::v1::pb::{RevUdpReq, RevUdpRes};
use futures::StreamExt;
use std::io::Error;
use tokio::sync::mpsc::Sender;
use tonic::Streaming;

/// 反向代理 listener 侧的 UDP 写端：向 connector 发送 UDP 数据包（RevUdpReq）
pub struct RevGrpcUdpClientWriter {
    writer: Sender<RevUdpReq>,
    auth: String,
}

/// 反向代理 listener 侧的 UDP 读端：从 connector 接收 UDP 响应（RevUdpRes）
pub struct RevGrpcUdpClientReader {
    reader: Streaming<RevUdpRes>,
}

impl RevGrpcUdpClientWriter {
    pub fn new(writer: Sender<RevUdpReq>, auth: String) -> Self {
        Self { writer, auth }
    }
}

impl RevGrpcUdpClientReader {
    pub fn new(reader: Streaming<RevUdpRes>) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for RevGrpcUdpClientWriter {
    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let req = RevUdpReq::from_packet(packet, self.auth.clone());
        self.writer
            .send(req)
            .await
            .map_err(|e| Error::new(std::io::ErrorKind::BrokenPipe, e.to_string()))
    }
}

#[async_trait::async_trait]
impl RunUdpReader for RevGrpcUdpClientReader {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        match self.reader.next().await {
            Some(Ok(res)) => res
                .try_into()
                .map_err(|_| Error::other("rev grpc udp res convert error")),
            Some(Err(e)) => Err(Error::new(std::io::ErrorKind::BrokenPipe, e.to_string())),
            None => Err(Error::other("rev grpc udp stream closed")),
        }
    }
}
