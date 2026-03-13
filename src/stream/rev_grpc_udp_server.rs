use crate::def::{RunUdpReader, RunUdpWriter, UDPPacket};
use crate::proto::v1::pb::{RevUdpReq, RevUdpRes};
use futures::StreamExt;
use std::io::ErrorKind;
use tokio::sync::mpsc::Sender;
use tonic::{Status, Streaming};

/// 反向代理 connector 侧的 UDP 读端：从 listener 接收 UDP 数据包（RevUdpReq）
pub struct RevGrpcUdpServerReader {
    reader: Streaming<RevUdpReq>,
    auth: String,
}

/// 反向代理 connector 侧的 UDP 写端：向 listener 发送 UDP 响应（RevUdpRes）
pub struct RevGrpcUdpServerWriter {
    writer: Sender<Result<RevUdpRes, Status>>,
}

impl RevGrpcUdpServerReader {
    pub fn new(reader: Streaming<RevUdpReq>, auth: String) -> Self {
        Self { reader, auth }
    }
}

impl RevGrpcUdpServerWriter {
    pub fn new(writer: Sender<Result<RevUdpRes, Status>>) -> Self {
        Self { writer }
    }
}

#[async_trait::async_trait]
impl RunUdpReader for RevGrpcUdpServerReader {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        let res = self.reader.next().await;
        match res {
            None => Err(std::io::Error::other("rev grpc udp stream closed")),
            Some(Err(e)) => Err(std::io::Error::new(ErrorKind::BrokenPipe, e.to_string())),
            Some(Ok(req)) => {
                if req.auth != self.auth {
                    return Err(std::io::Error::other("rev grpc udp auth mismatch"));
                }
                req.try_into()
                    .map_err(|_| std::io::Error::other("rev grpc udp req convert error"))
            }
        }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for RevGrpcUdpServerWriter {
    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let res: RevUdpRes = packet
            .try_into()
            .map_err(|_| std::io::Error::other("rev grpc udp res convert error"))?;
        self.writer
            .send(Ok(res))
            .await
            .map_err(|e| std::io::Error::new(ErrorKind::BrokenPipe, e.to_string()))
    }
}
