use crate::def::{RunReadHalf, RunStream, RunWriteHalf, StreamInfo};
use crate::proto::v1::pb::{StreamReq, StreamRes};
use crate::util::crypto::{decrypt_bytes, encrypt_bytes};
use crate::util::pb_http::{PbHttpOptions, read_message, send_message};
use bytes::{Bytes, BytesMut};
use h2::{RecvStream, SendStream};
use std::any::Any;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct PbHttpClientReadHalf {
    reader: Arc<Mutex<RecvStream>>,
    frame_buf: BytesMut,
    cache: Vec<u8>,
    cache_pos: usize,
    pw: String,
    encrypt: bool,
    options: PbHttpOptions,
}

pub struct PbHttpClientWriteHalf {
    writer: Arc<Mutex<SendStream<Bytes>>>,
    pw: String,
    encrypt: bool,
    options: PbHttpOptions,
}

pub struct PbHttpClientRunStream {
    reader: Arc<Mutex<RecvStream>>,
    writer: Arc<Mutex<SendStream<Bytes>>>,
    frame_buf: BytesMut,
    cache: Vec<u8>,
    cache_pos: usize,
    info: StreamInfo,
    pw: String,
    encrypt: bool,
    options: PbHttpOptions,
}

impl PbHttpClientRunStream {
    pub fn new(
        reader: RecvStream,
        writer: SendStream<Bytes>,
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
    reader: &Arc<Mutex<RecvStream>>,
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
    let msg: Option<StreamRes> = read_message(&mut r, frame_buf, options.max_message_size).await?;
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
    writer: &Arc<Mutex<SendStream<Bytes>>>,
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
    send_message(&mut w, &req, options.send_chunk_size).await
}

#[async_trait::async_trait]
impl RunReadHalf for PbHttpClientReadHalf {
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
impl RunWriteHalf for PbHttpClientWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> std::io::Result<()> {
        write_payload(&self.writer, buf, &self.pw, self.encrypt, &self.options).await
    }
}

#[async_trait::async_trait]
impl RunStream for PbHttpClientRunStream {
    fn get_info(&self) -> &StreamInfo {
        &self.info
    }

    fn set_info(&mut self, f: &mut dyn FnMut(&mut StreamInfo)) {
        f(&mut self.info)
    }

    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        (
            Box::new(PbHttpClientReadHalf {
                reader: Arc::clone(&self.reader),
                frame_buf: BytesMut::new(),
                cache: Vec::new(),
                cache_pos: 0,
                pw: self.pw.clone(),
                encrypt: self.encrypt,
                options: self.options.clone(),
            }),
            Box::new(PbHttpClientWriteHalf {
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
