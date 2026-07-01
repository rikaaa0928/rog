use crate::def::{RunReadHalf, RunStream, RunWriteHalf, StreamInfo};
use crate::proto::v1::pb::{StreamReq, StreamRes};
use crate::util::crypto::{decrypt_bytes, encrypt_bytes};
use crate::util::pb_http::{
    H3ClientRecvStream, H3ClientSendStream, PbHttpOptions, read_h3_message, send_h3_message,
};
use bytes::BytesMut;
use std::any::Any;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct PbHttpH3ClientReadHalf {
    reader: Arc<Mutex<H3ClientRecvStream>>,
    frame_buf: BytesMut,
    cache: Vec<u8>,
    cache_pos: usize,
    pw: String,
    encrypt: bool,
    options: PbHttpOptions,
}

pub struct PbHttpH3ClientWriteHalf {
    writer: Arc<Mutex<H3ClientSendStream>>,
    pw: String,
    encrypt: bool,
    options: PbHttpOptions,
}

pub struct PbHttpH3ClientRunStream {
    reader: Arc<Mutex<H3ClientRecvStream>>,
    writer: Arc<Mutex<H3ClientSendStream>>,
    frame_buf: BytesMut,
    cache: Vec<u8>,
    cache_pos: usize,
    info: StreamInfo,
    pw: String,
    encrypt: bool,
    options: PbHttpOptions,
}

impl PbHttpH3ClientRunStream {
    pub fn new(
        reader: H3ClientRecvStream,
        writer: H3ClientSendStream,
        pw: String,
        encrypt: bool,
        options: PbHttpOptions,
    ) -> Self {
        Self {
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            frame_buf: BytesMut::new(),
            cache: Vec::new(),
            cache_pos: 0,
            info: StreamInfo::default(),
            pw,
            encrypt,
            options,
        }
    }
}

async fn read_payload(
    reader: &Arc<Mutex<H3ClientRecvStream>>,
    frame_buf: &mut BytesMut,
    cache: &mut Vec<u8>,
    cache_pos: &mut usize,
    buf: &mut [u8],
    pw: &str,
    encrypt: bool,
    options: &PbHttpOptions,
) -> std::io::Result<usize> {
    if *cache_pos < cache.len() {
        let available = cache.len() - *cache_pos;
        let to_copy = available.min(buf.len());
        buf[..to_copy].copy_from_slice(&cache[*cache_pos..*cache_pos + to_copy]);
        *cache_pos += to_copy;
        if *cache_pos >= cache.len() {
            cache.clear();
            *cache_pos = 0;
        }
        return Ok(to_copy);
    }

    cache.clear();
    *cache_pos = 0;

    let mut r = reader.lock().await;
    let msg: Option<StreamRes> =
        read_h3_message(&mut r, frame_buf, options.max_message_size).await?;
    let Some(msg) = msg else {
        return Ok(0);
    };
    let mut payload = msg.payload;
    if payload.is_empty() {
        return Ok(0);
    }

    if encrypt {
        payload = decrypt_bytes(&payload, pw)?;
    }

    let to_copy = buf.len().min(payload.len());
    buf[..to_copy].copy_from_slice(&payload[..to_copy]);
    if payload.len() > to_copy {
        cache.extend_from_slice(&payload[to_copy..]);
    }
    Ok(to_copy)
}

async fn write_payload(
    writer: &Arc<Mutex<H3ClientSendStream>>,
    buf: &[u8],
    pw: &str,
    encrypt: bool,
    options: &PbHttpOptions,
) -> std::io::Result<()> {
    let payload = if encrypt {
        encrypt_bytes(buf, pw)?
    } else {
        buf.to_vec()
    };
    let req = StreamReq {
        payload: Some(payload),
        ..Default::default()
    };
    let mut w = writer.lock().await;
    send_h3_message(&mut w, &req, options.send_chunk_size).await
}

#[async_trait::async_trait]
impl RunReadHalf for PbHttpH3ClientReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        read_payload(
            &self.reader,
            &mut self.frame_buf,
            &mut self.cache,
            &mut self.cache_pos,
            buf,
            &self.pw,
            self.encrypt,
            &self.options,
        )
        .await
    }
}

#[async_trait::async_trait]
impl RunWriteHalf for PbHttpH3ClientWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        write_payload(&self.writer, buf, &self.pw, self.encrypt, &self.options).await
    }
}

#[async_trait::async_trait]
impl RunStream for PbHttpH3ClientRunStream {
    fn get_info(&self) -> &StreamInfo {
        &self.info
    }

    fn set_info(&mut self, f: &mut dyn FnMut(&mut StreamInfo)) {
        f(&mut self.info)
    }

    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        (
            Box::new(PbHttpH3ClientReadHalf {
                reader: Arc::clone(&self.reader),
                frame_buf: BytesMut::new(),
                cache: Vec::new(),
                cache_pos: 0,
                pw: self.pw.clone(),
                encrypt: self.encrypt,
                options: self.options.clone(),
            }),
            Box::new(PbHttpH3ClientWriteHalf {
                writer: Arc::clone(&self.writer),
                pw: self.pw.clone(),
                encrypt: self.encrypt,
                options: self.options.clone(),
            }),
        )
    }

    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        read_payload(
            &self.reader,
            &mut self.frame_buf,
            &mut self.cache,
            &mut self.cache_pos,
            buf,
            &self.pw,
            self.encrypt,
            &self.options,
        )
        .await
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        write_payload(&self.writer, buf, &self.pw, self.encrypt, &self.options).await
    }
}
