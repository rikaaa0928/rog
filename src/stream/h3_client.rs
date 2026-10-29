use bytes::{Buf, Bytes};
use std::any::Any;
use std::io::{self, ErrorKind};
use tokio::sync::Mutex;

use crate::def::{
    RunReadHalf, RunStream, RunUdpReader, RunUdpWriter, RunWriteHalf, StreamInfo, UDPPacket,
};
use crate::util::h3_frame::{
    LpmDecoder, decode_udp_packet_frame, encode_lpm_frame, encode_udp_packet_frame,
};

pub enum ClientSendBodyInner {
    H3(h3::client::RequestStream<h3_quinn::SendStream<Bytes>, Bytes>),
    H2(h2::SendStream<Bytes>),
}

pub struct ClientSendBody {
    inner: Option<ClientSendBodyInner>,
}

impl ClientSendBody {
    pub fn new_h3(stream: h3::client::RequestStream<h3_quinn::SendStream<Bytes>, Bytes>) -> Self {
        Self {
            inner: Some(ClientSendBodyInner::H3(stream)),
        }
    }

    pub fn new_h2(stream: h2::SendStream<Bytes>) -> Self {
        Self {
            inner: Some(ClientSendBodyInner::H2(stream)),
        }
    }

    pub async fn send_raw_bytes(&mut self, mut data: Bytes) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        match self.inner.as_mut() {
            Some(ClientSendBodyInner::H3(stream)) => stream
                .send_data(data)
                .await
                .map_err(|e| io::Error::new(ErrorKind::BrokenPipe, format!("h3 send error: {e}"))),
            Some(ClientSendBodyInner::H2(stream)) => {
                while !data.is_empty() {
                    let want = data.len();
                    stream.reserve_capacity(want);
                    match std::future::poll_fn(|cx| stream.poll_capacity(cx)).await {
                        Some(Ok(cap)) => {
                            let chunk = data.split_to(want.min(cap));
                            stream.send_data(chunk, false).map_err(|e| {
                                io::Error::new(ErrorKind::BrokenPipe, format!("h2 send error: {e}"))
                            })?;
                        }
                        Some(Err(e)) => {
                            return Err(io::Error::new(
                                ErrorKind::BrokenPipe,
                                format!("h2 poll_capacity error: {e}"),
                            ));
                        }
                        None => {
                            return Err(io::Error::new(
                                ErrorKind::BrokenPipe,
                                "h2 send stream closed",
                            ));
                        }
                    }
                }
                Ok(())
            }
            None => Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "client send stream already finished",
            )),
        }
    }

    pub async fn send_lpm_payload(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() {
            return Ok(());
        }
        let frame = encode_lpm_frame(payload);
        self.send_raw_bytes(frame).await
    }

    pub async fn finish(&mut self) -> io::Result<()> {
        if let Some(inner) = self.inner.take() {
            match inner {
                ClientSendBodyInner::H3(mut stream) => {
                    let _ = stream.finish().await;
                }
                ClientSendBodyInner::H2(mut stream) => {
                    let _ = stream.send_data(Bytes::new(), true);
                }
            }
        }
        Ok(())
    }
}

impl Drop for ClientSendBody {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            match inner {
                ClientSendBodyInner::H3(mut stream) => {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.spawn(async move {
                            let _ = stream.finish().await;
                        });
                    }
                }
                ClientSendBodyInner::H2(mut stream) => {
                    let _ = stream.send_data(Bytes::new(), true);
                }
            }
        }
    }
}

pub enum ClientRecvBody {
    H3 {
        stream: h3::client::RequestStream<h3_quinn::RecvStream, Bytes>,
        headers_received: bool,
    },
    H2 {
        response_fut: Option<h2::client::ResponseFuture>,
        body: Option<h2::RecvStream>,
    },
}

impl Drop for ClientRecvBody {
    fn drop(&mut self) {
        if let ClientRecvBody::H2 { body, .. } = self
            && let Some(mut stream) = body.take()
            && !stream.is_end_stream()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            handle.spawn(async move {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async move {
                    while let Some(Ok(chunk)) = stream.data().await {
                        let _ = stream.flow_control().release_capacity(chunk.len());
                    }
                })
                .await;
            });
        }
    }
}

impl ClientRecvBody {
    pub async fn recv_chunk(&mut self) -> io::Result<Option<Bytes>> {
        match self {
            ClientRecvBody::H3 {
                stream,
                headers_received,
            } => {
                if !*headers_received {
                    let resp = stream.recv_response().await.map_err(|e| {
                        io::Error::new(
                            ErrorKind::ConnectionAborted,
                            format!("h3 recv_response error: {e}"),
                        )
                    })?;
                    if resp.status() != http::StatusCode::OK {
                        return Err(io::Error::new(
                            ErrorKind::PermissionDenied,
                            format!("h3 server returned HTTP {}", resp.status()),
                        ));
                    }
                    *headers_received = true;
                }
                match stream.recv_data().await {
                    Ok(Some(mut buf)) => {
                        let len = buf.remaining();
                        if len == 0 {
                            Ok(Some(Bytes::new()))
                        } else {
                            Ok(Some(buf.copy_to_bytes(len)))
                        }
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(io::Error::new(
                        ErrorKind::ConnectionReset,
                        format!("h3 recv_data error: {e}"),
                    )),
                }
            }
            ClientRecvBody::H2 { response_fut, body } => {
                if body.is_none() {
                    let fut = response_fut
                        .take()
                        .ok_or_else(|| io::Error::other("missing h2 response future"))?;
                    let resp = fut.await.map_err(|e| {
                        io::Error::new(
                            ErrorKind::ConnectionAborted,
                            format!("h2 response header error: {e}"),
                        )
                    })?;
                    if resp.status() != http::StatusCode::OK {
                        return Err(io::Error::new(
                            ErrorKind::PermissionDenied,
                            format!("h2 server returned HTTP {}", resp.status()),
                        ));
                    }
                    *body = Some(resp.into_body());
                }
                let recv_stream = body.as_mut().unwrap();
                match recv_stream.data().await {
                    Some(Ok(chunk)) => {
                        let _ = recv_stream.flow_control().release_capacity(chunk.len());
                        Ok(Some(chunk))
                    }
                    Some(Err(e)) => Err(io::Error::new(
                        ErrorKind::ConnectionReset,
                        format!("h2 recv data error: {e}"),
                    )),
                    None => Ok(None),
                }
            }
        }
    }
}

pub struct H3ClientReadHalf {
    recv: ClientRecvBody,
    decoder: LpmDecoder,
    current_frame: Bytes,
}

impl H3ClientReadHalf {
    pub fn new(recv: ClientRecvBody) -> Self {
        Self {
            recv,
            decoder: LpmDecoder::new(),
            current_frame: Bytes::new(),
        }
    }
}

#[async_trait::async_trait]
impl RunReadHalf for H3ClientReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if !self.current_frame.is_empty() {
                let to_copy = buf.len().min(self.current_frame.len());
                buf[..to_copy].copy_from_slice(&self.current_frame[..to_copy]);
                self.current_frame.advance(to_copy);
                return Ok(to_copy);
            }

            if let Some(frame) = self.decoder.next_frame()? {
                if frame.is_empty() {
                    continue;
                }
                self.current_frame = frame;
                continue;
            }

            match self.recv.recv_chunk().await? {
                Some(chunk) => {
                    if !chunk.is_empty() {
                        self.decoder.push(&chunk);
                    }
                }
                None => return Ok(0),
            }
        }
    }
}

pub struct H3ClientWriteHalf {
    send: ClientSendBody,
}

impl H3ClientWriteHalf {
    pub fn new(send: ClientSendBody) -> Self {
        Self { send }
    }
}

#[async_trait::async_trait]
impl RunWriteHalf for H3ClientWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        self.send.send_lpm_payload(buf).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.send.finish().await
    }
}

pub struct H3ClientRunStream {
    read_half: H3ClientReadHalf,
    write_half: H3ClientWriteHalf,
    info: StreamInfo,
}

impl H3ClientRunStream {
    pub fn new(recv: ClientRecvBody, send: ClientSendBody, info: StreamInfo) -> Self {
        Self {
            read_half: H3ClientReadHalf::new(recv),
            write_half: H3ClientWriteHalf::new(send),
            info,
        }
    }
}

#[async_trait::async_trait]
impl RunStream for H3ClientRunStream {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn get_info(&self) -> &StreamInfo {
        &self.info
    }

    fn set_info(&mut self, f: &mut dyn FnMut(&mut StreamInfo)) {
        f(&mut self.info)
    }

    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        (Box::new(self.read_half), Box::new(self.write_half))
    }

    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_half.read(buf).await
    }

    async fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write_half.write(buf).await
    }
}

struct UdpWriteState {
    send: ClientSendBody,
    last_dst: Option<(String, u16)>,
}

pub struct H3UdpClientRunWriter {
    state: Mutex<UdpWriteState>,
}

impl H3UdpClientRunWriter {
    pub fn new(send: ClientSendBody) -> Self {
        Self {
            state: Mutex::new(UdpWriteState {
                send,
                last_dst: None,
            }),
        }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for H3UdpClientRunWriter {
    async fn write(&self, packet: UDPPacket) -> io::Result<()> {
        let mut guard = self.state.lock().await;
        let frame = encode_udp_packet_frame(
            &packet.meta.dst_addr,
            packet.meta.dst_port,
            &mut guard.last_dst,
            &packet.data,
        )?;
        guard.send.send_raw_bytes(frame).await
    }
}

pub struct H3UdpClientRunReader {
    recv: ClientRecvBody,
    decoder: LpmDecoder,
    last_dst: Option<(String, u16)>,
    src_host: String,
    src_port: u16,
}

impl H3UdpClientRunReader {
    pub fn new(recv: ClientRecvBody, src_host: String, src_port: u16) -> Self {
        Self {
            recv,
            decoder: LpmDecoder::new(),
            last_dst: None,
            src_host,
            src_port,
        }
    }
}

#[async_trait::async_trait]
impl RunUdpReader for H3UdpClientRunReader {
    async fn read(&mut self) -> io::Result<UDPPacket> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return decode_udp_packet_frame(
                    frame,
                    &mut self.last_dst,
                    &self.src_host,
                    self.src_port,
                );
            }
            match self.recv.recv_chunk().await? {
                Some(chunk) => {
                    if !chunk.is_empty() {
                        self.decoder.push(&chunk);
                    }
                }
                None => {
                    return Err(io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "h3 UDP stream closed",
                    ));
                }
            }
        }
    }
}
