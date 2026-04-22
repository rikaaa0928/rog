use crate::def::{RunUdpReader, RunUdpWriter, UDPPacket};
use crate::proto::v1::pb::{UdpReq, UdpRes};
use crate::util::crypto::{decrypt_bytes, decrypt_field, encrypt_bytes, encrypt_field};
use crate::util::tcp_frame::{read_msg, write_frame};
use std::sync::Arc;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;

pub struct PbTcpUdpClientWriter {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    auth: String,
    pw: String,
}

pub struct PbTcpUdpClientReader {
    reader: Arc<Mutex<OwnedReadHalf>>,
    pw: String,
}

impl PbTcpUdpClientWriter {
    pub fn new(writer: Arc<Mutex<OwnedWriteHalf>>, auth: String, pw: String) -> Self {
        Self { writer, auth, pw }
    }
}

impl PbTcpUdpClientReader {
    pub fn new(reader: Arc<Mutex<OwnedReadHalf>>, pw: String) -> Self {
        Self { reader, pw }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for PbTcpUdpClientWriter {
    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let mut req = UdpReq::from_packet(packet, self.auth.clone());
        req.auth = encrypt_field(&req.auth, &self.pw)?;
        if let Some(ref v) = req.dst_addr {
            req.dst_addr = Some(encrypt_field(v, &self.pw)?);
        }
        if let Some(ref v) = req.src_addr {
            req.src_addr = Some(encrypt_field(v, &self.pw)?);
        }
        if let Some(payload) = req.payload {
            req.payload = Some(encrypt_bytes(&payload, &self.pw)?);
        }
        let mut w = self.writer.lock().await;
        write_frame(&mut *w, &req).await
    }
}

#[async_trait::async_trait]
impl RunUdpReader for PbTcpUdpClientReader {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        let mut r = self.reader.lock().await;
        let mut res: UdpRes = read_msg(&mut *r).await?;
        if let Some(ref enc) = res.dst_addr {
            res.dst_addr = Some(decrypt_field(enc, &self.pw)?);
        }
        if let Some(ref enc) = res.src_addr {
            res.src_addr = Some(decrypt_field(enc, &self.pw)?);
        }
        res.payload = decrypt_bytes(&res.payload, &self.pw)?;
        res.try_into()
            .map_err(|_| std::io::Error::other("pb tcp udp res convert error"))
    }
}
