use crate::def::{RunUdpReader, RunUdpWriter, UDPPacket};
use crate::proto::v1::pb::{UdpReq, UdpRes};
use crate::util::crypto::{decrypt_bytes, decrypt_field, encrypt_bytes, encrypt_field};
use crate::util::pb_http::{
    H3ClientRecvStream, H3ClientSendStream, PbHttpOptions, read_h3_message, send_h3_message,
};
use bytes::BytesMut;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct PbHttpH3UdpClientWriter {
    writer: Arc<Mutex<H3ClientSendStream>>,
    auth: String,
    pw: String,
    options: PbHttpOptions,
}

pub struct PbHttpH3UdpClientReader {
    reader: Arc<Mutex<H3ClientRecvStream>>,
    frame_buf: BytesMut,
    pw: String,
    options: PbHttpOptions,
}

impl PbHttpH3UdpClientWriter {
    pub fn new(
        writer: Arc<Mutex<H3ClientSendStream>>,
        auth: String,
        pw: String,
        options: PbHttpOptions,
    ) -> Self {
        Self {
            writer,
            auth,
            pw,
            options,
        }
    }
}

impl PbHttpH3UdpClientReader {
    pub fn new(reader: Arc<Mutex<H3ClientRecvStream>>, pw: String, options: PbHttpOptions) -> Self {
        Self {
            reader,
            frame_buf: BytesMut::new(),
            pw,
            options,
        }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for PbHttpH3UdpClientWriter {
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
        send_h3_message(&mut w, &req, self.options.send_chunk_size).await
    }
}

#[async_trait::async_trait]
impl RunUdpReader for PbHttpH3UdpClientReader {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        let mut r = self.reader.lock().await;
        let mut res: UdpRes =
            read_h3_message(&mut r, &mut self.frame_buf, self.options.max_message_size)
                .await?
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "pb_http h3 udp eof")
                })?;
        if let Some(ref enc) = res.dst_addr {
            res.dst_addr = Some(decrypt_field(enc, &self.pw)?);
        }
        if let Some(ref enc) = res.src_addr {
            res.src_addr = Some(decrypt_field(enc, &self.pw)?);
        }
        res.payload = decrypt_bytes(&res.payload, &self.pw)?;
        res.try_into()
            .map_err(|_| std::io::Error::other("pb_http h3 udp res convert error"))
    }
}
