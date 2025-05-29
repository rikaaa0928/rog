use crate::def::{
    RunUdpReader, RunUdpWriter, UDPPacket,
};
use crate::proto::v1::pb::{UdpReq, UdpRes};
use futures::StreamExt;
use std::io::ErrorKind;
use tokio::sync::mpsc::Sender;
use tonic::{Status, Streaming};
// empty stream
pub struct GrpcUdpServerReadHalf {
    reader: Streaming<UdpReq>,
    auth: String,
}

pub struct GrpcUdpServerWriteHalf {
    writer: Sender<Result<UdpRes, Status>>,
}
impl GrpcUdpServerReadHalf {
    pub fn new(reader: Streaming<UdpReq>, auth: String) -> Self {
        Self { reader, auth }
    }
}

impl GrpcUdpServerWriteHalf {
    pub fn new(writer: Sender<Result<UdpRes, Status>>) -> Self {
        Self { writer }
    }
}

#[async_trait::async_trait]
impl RunUdpReader for GrpcUdpServerReadHalf {
    // async fn recv_from(&mut self, buf: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
    //     let res = self.reader.next().await;
    //     if res.is_none() {
    //         return Err(std::io::Error::new(ErrorKind::Other, "no more data"));
    //     }
    //     let res = res.unwrap();
    //     match res {
    //         Ok(data) => {
    //             if self.auth != data.auth {
    //                 return Err(std::io::Error::new(ErrorKind::Other, "auth mismatch"));
    //             }
    //             let n = data.payload.as_ref().unwrap().len();
    //             buf[..n].copy_from_slice(data.payload.as_ref().unwrap());
    //             let addr = format!("{}:{}", data.src_addr.unwrap(), data.src_port.unwrap());
    //             Ok((n, addr.parse().unwrap()))
    //         }
    //         Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
    //     }
    // }

    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        let res = self.reader.next().await;
        if res.is_none() {
            return Err(std::io::Error::new(ErrorKind::Other, "no more data"));
        }
        let res = res.unwrap();
        match res {
            Ok(data) => {
                if self.auth != data.auth {
                    return Err(std::io::Error::new(ErrorKind::Other, "auth mismatch"));
                }
                // let n = data.payload.as_ref().unwrap().len();
                // buf[..n].copy_from_slice(data.payload.as_ref().unwrap());
                // let addr = format!("{}:{}", data.src_addr.unwrap(), data.src_port.unwrap());
                Ok(data.try_into().unwrap())
            }
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for GrpcUdpServerWriteHalf {
    // async fn send_to(&self, buf: &[u8], _: String) -> std::io::Result<usize> {
    //     let mut res = UdpRes::default();
    //     res.payload = buf.to_vec();
    //     match self.writer.send(Ok(res)).await {
    //         Ok(_) => Ok(1),
    //         Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
    //     }
    // }

    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let res = packet.try_into().unwrap();
        match self.writer.send(Ok(res)).await {
            Ok(_) => Ok(()),
            Err(e) => Err(std::io::Error::new(ErrorKind::Interrupted, e.to_string())),
        }
    }
}
