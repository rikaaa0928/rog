use crate::def::{RunUdpReader, RunUdpWriter, UDPPacket};
use crate::proto::v1::pb::{UdpReq, UdpRes};
use crate::util::crypto::{decrypt_field, encrypt_field};
use crate::util::tcp_frame::{read_msg, write_frame};
use std::sync::Arc;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;

pub struct PbTcpUdpServerReader {
    reader: Arc<Mutex<OwnedReadHalf>>,
    auth: String,
    pw: String,
}

pub struct PbTcpUdpServerWriter {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    pw: String,
}

impl PbTcpUdpServerReader {
    pub fn new(reader: Arc<Mutex<OwnedReadHalf>>, auth: String, pw: String) -> Self {
        Self { reader, auth, pw }
    }
}

impl PbTcpUdpServerWriter {
    pub fn new(writer: Arc<Mutex<OwnedWriteHalf>>, pw: String) -> Self {
        Self { writer, pw }
    }
}

#[async_trait::async_trait]
impl RunUdpReader for PbTcpUdpServerReader {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        let mut r = self.reader.lock().await;
        let mut req: UdpReq = read_msg(&mut *r).await?;
        let decrypted_auth = decrypt_field(&req.auth, &self.pw)?;
        if decrypted_auth != self.auth {
            return Err(std::io::Error::other("pb tcp udp auth mismatch"));
        }
        if let Some(ref enc) = req.dst_addr {
            req.dst_addr = Some(decrypt_field(enc, &self.pw)?);
        }
        if let Some(ref enc) = req.src_addr {
            req.src_addr = Some(decrypt_field(enc, &self.pw)?);
        }
        req.try_into()
            .map_err(|_| std::io::Error::other("pb tcp udp req convert error"))
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for PbTcpUdpServerWriter {
    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let mut res: UdpRes = packet
            .try_into()
            .map_err(|_| std::io::Error::other("pb tcp udp res convert error"))?;
        if let Some(ref v) = res.dst_addr {
            res.dst_addr = Some(encrypt_field(v, &self.pw)?);
        }
        if let Some(ref v) = res.src_addr {
            res.src_addr = Some(encrypt_field(v, &self.pw)?);
        }
        let mut w = self.writer.lock().await;
        write_frame(&mut *w, &res).await
    }
}
