use crate::def::{RunUdpReader, RunUdpWriter, UDPPacket};
use crate::proto::v1::pb::{UdpReq, UdpRes};
use crate::util::crypto::{decrypt_bytes, decrypt_field, encrypt_bytes, encrypt_field};
use crate::util::pb_http::{PbHttpOptions, read_message, send_message};
use bytes::{Bytes, BytesMut};
use h2::{RecvStream, SendStream};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct PbHttpUdpServerReader {
    reader: Arc<Mutex<RecvStream>>,
    frame_buf: BytesMut,
    auth: String,
    pw: String,
    options: PbHttpOptions,
}

pub struct PbHttpUdpServerWriter {
    writer: Arc<Mutex<SendStream<Bytes>>>,
    pw: String,
    options: PbHttpOptions,
}

impl PbHttpUdpServerReader {
    pub fn new(
        reader: Arc<Mutex<RecvStream>>,
        auth: String,
        pw: String,
        options: PbHttpOptions,
    ) -> Self {
        Self {
            reader,
            frame_buf: BytesMut::new(),
            auth,
            pw,
            options,
        }
    }
}

impl PbHttpUdpServerWriter {
    pub fn new(writer: Arc<Mutex<SendStream<Bytes>>>, pw: String, options: PbHttpOptions) -> Self {
        Self {
            writer,
            pw,
            options,
        }
    }
}

#[async_trait::async_trait]
impl RunUdpReader for PbHttpUdpServerReader {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        let mut r = self.reader.lock().await;
        let mut req: UdpReq =
            read_message(&mut r, &mut self.frame_buf, self.options.max_message_size)
                .await?
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "pb_http udp eof")
                })?;
        let decrypted_auth = decrypt_field(&req.auth, &self.pw)?;
        if decrypted_auth != self.auth {
            return Err(std::io::Error::other("pb http udp auth mismatch"));
        }
        if let Some(ref enc) = req.dst_addr {
            req.dst_addr = Some(decrypt_field(enc, &self.pw)?);
        }
        if let Some(ref enc) = req.src_addr {
            req.src_addr = Some(decrypt_field(enc, &self.pw)?);
        }
        if let Some(payload) = req.payload {
            req.payload = Some(decrypt_bytes(&payload, &self.pw)?);
        }
        req.try_into()
            .map_err(|_| std::io::Error::other("pb http udp req convert error"))
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for PbHttpUdpServerWriter {
    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let mut res: UdpRes = packet
            .try_into()
            .map_err(|_| std::io::Error::other("pb http udp res convert error"))?;
        if let Some(ref v) = res.dst_addr {
            res.dst_addr = Some(encrypt_field(v, &self.pw)?);
        }
        if let Some(ref v) = res.src_addr {
            res.src_addr = Some(encrypt_field(v, &self.pw)?);
        }
        res.payload = encrypt_bytes(&res.payload, &self.pw)?;
        let mut w = self.writer.lock().await;
        send_message(&mut w, &res, self.options.send_chunk_size).await
    }
}
