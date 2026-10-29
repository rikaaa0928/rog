use bytes::{Buf, Bytes};
use prost::Message;
use std::any::Any;
use std::io::{self, ErrorKind};
use tokio::sync::Mutex;

use crate::def::{
    RunReadHalf, RunStream, RunUdpReader, RunUdpWriter, RunWriteHalf, StreamInfo, UDPPacket,
};
use crate::proto::v1::pb::{StreamReq, StreamRes, UdpReq, UdpRes};
use crate::util::RunAddr;
use crate::util::h3_frame::{
    LpmDecoder, decode_tcp_handshake_frame, decode_udp_handshake_frame, decode_udp_packet_frame,
    encode_lpm_frame, encode_udp_packet_frame,
};

pub enum ServerSendBody {
    H3(h3::server::RequestStream<h3_quinn::SendStream<Bytes>, Bytes>),
    H2(h2::SendStream<Bytes>),
}

impl ServerSendBody {
    pub async fn send_raw_bytes(&mut self, mut data: Bytes) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        match self {
            ServerSendBody::H3(stream) => stream.send_data(data).await.map_err(|e| {
                io::Error::new(ErrorKind::BrokenPipe, format!("h3 server send error: {e}"))
            }),
            ServerSendBody::H2(stream) => {
                while !data.is_empty() {
                    let want = data.len();
                    stream.reserve_capacity(want);
                    match std::future::poll_fn(|cx| stream.poll_capacity(cx)).await {
                        Some(Ok(cap)) => {
                            let chunk = data.split_to(want.min(cap));
                            stream.send_data(chunk, false).map_err(|e| {
                                io::Error::new(
                                    ErrorKind::BrokenPipe,
                                    format!("h2 server send error: {e}"),
                                )
                            })?;
                        }
                        Some(Err(e)) => {
                            return Err(io::Error::new(
                                ErrorKind::BrokenPipe,
                                format!("h2 server poll_capacity error: {e}"),
                            ));
                        }
                        None => {
                            return Err(io::Error::new(
                                ErrorKind::BrokenPipe,
                                "h2 server send stream closed",
                            ));
                        }
                    }
                }
                Ok(())
            }
        }
    }

    pub async fn send_lpm_payload(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() {
            return Ok(());
        }
        let frame = encode_lpm_frame(payload);
        self.send_raw_bytes(frame).await
    }
}

pub enum ServerRecvBody {
    H3(h3::server::RequestStream<h3_quinn::RecvStream, Bytes>),
    H2(h2::RecvStream),
}

impl ServerRecvBody {
    pub async fn recv_chunk(&mut self) -> io::Result<Option<Bytes>> {
        match self {
            ServerRecvBody::H3(stream) => match stream.recv_data().await {
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
                    format!("h3 server recv_data error: {e}"),
                )),
            },
            ServerRecvBody::H2(stream) => match stream.data().await {
                Some(Ok(chunk)) => {
                    let _ = stream.flow_control().release_capacity(chunk.len());
                    Ok(Some(chunk))
                }
                Some(Err(e)) => Err(io::Error::new(
                    ErrorKind::ConnectionReset,
                    format!("h2 server recv data error: {e}"),
                )),
                None => Ok(None),
            },
        }
    }
}

pub struct H3ServerReadHalf {
    recv: ServerRecvBody,
    decoder: LpmDecoder,
    current_frame: Bytes,
    legacy_pb: bool,
}

impl H3ServerReadHalf {
    pub fn new(recv: ServerRecvBody, legacy_pb: bool) -> Self {
        Self {
            recv,
            decoder: LpmDecoder::new(),
            current_frame: Bytes::new(),
            legacy_pb,
        }
    }

    pub async fn read_next_lpm_frame(&mut self) -> io::Result<Option<Bytes>> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(Some(frame));
            }
            match self.recv.recv_chunk().await? {
                Some(chunk) => {
                    if !chunk.is_empty() {
                        self.decoder.push(&chunk);
                    }
                }
                None => return Ok(None),
            }
        }
    }
}

#[async_trait::async_trait]
impl RunReadHalf for H3ServerReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if !self.current_frame.is_empty() {
                let to_copy = buf.len().min(self.current_frame.len());
                buf[..to_copy].copy_from_slice(&self.current_frame[..to_copy]);
                self.current_frame.advance(to_copy);
                return Ok(to_copy);
            }

            match self.read_next_lpm_frame().await? {
                Some(frame) => {
                    let payload = if self.legacy_pb {
                        let req = StreamReq::decode(frame).map_err(|e| {
                            io::Error::new(ErrorKind::InvalidData, format!("pb decode: {e}"))
                        })?;
                        Bytes::from(req.payload.unwrap_or_default())
                    } else {
                        frame
                    };
                    if payload.is_empty() {
                        continue;
                    }
                    self.current_frame = payload;
                }
                None => return Ok(0),
            }
        }
    }
}

pub struct H3ServerWriteHalf {
    send: ServerSendBody,
    legacy_pb: bool,
}

impl H3ServerWriteHalf {
    pub fn new(send: ServerSendBody, legacy_pb: bool) -> Self {
        Self { send, legacy_pb }
    }
}

#[async_trait::async_trait]
impl RunWriteHalf for H3ServerWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.legacy_pb {
            let res = StreamRes {
                payload: buf.to_vec(),
            };
            let encoded = res.encode_to_vec();
            self.send.send_lpm_payload(&encoded).await
        } else {
            self.send.send_lpm_payload(buf).await
        }
    }
}

pub struct H3ServerRunStream {
    read_half: H3ServerReadHalf,
    write_half: H3ServerWriteHalf,
    info: StreamInfo,
    legacy_pb: bool,
}

impl H3ServerRunStream {
    pub fn new(recv: ServerRecvBody, send: ServerSendBody, legacy_pb: bool) -> Self {
        Self {
            read_half: H3ServerReadHalf::new(recv, legacy_pb),
            write_half: H3ServerWriteHalf::new(send, legacy_pb),
            info: StreamInfo::default(),
            legacy_pb,
        }
    }

    pub async fn handshake(&mut self) -> io::Result<(RunAddr, String, Option<Vec<u8>>)> {
        let frame = self.read_half.read_next_lpm_frame().await?.ok_or_else(|| {
            io::Error::new(
                ErrorKind::UnexpectedEof,
                "EOF before H3 TCP handshake frame",
            )
        })?;
        if self.legacy_pb {
            let req = StreamReq::decode(frame).map_err(|e| {
                io::Error::new(ErrorKind::InvalidData, format!("pb handshake decode: {e}"))
            })?;
            let addr = RunAddr {
                addr: req.dst_addr.unwrap_or_default(),
                port: req.dst_port.unwrap_or(0) as u16,
                udp: false,
            };
            let initial = req.payload.filter(|p| !p.is_empty());
            Ok((addr, req.auth, initial))
        } else {
            let (auth, addr, initial_payload) = decode_tcp_handshake_frame(frame)?;
            Ok((addr, auth, initial_payload))
        }
    }
}

#[async_trait::async_trait]
impl RunStream for H3ServerRunStream {
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

struct ServerUdpWriteState {
    send: ServerSendBody,
    last_dst: Option<(String, u16)>,
    legacy_pb: bool,
}

pub struct H3UdpServerWriteHalf {
    state: Mutex<ServerUdpWriteState>,
}

impl H3UdpServerWriteHalf {
    pub fn new(send: ServerSendBody, legacy_pb: bool) -> Self {
        Self {
            state: Mutex::new(ServerUdpWriteState {
                send,
                last_dst: None,
                legacy_pb,
            }),
        }
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for H3UdpServerWriteHalf {
    async fn write(&self, packet: UDPPacket) -> io::Result<()> {
        let mut guard = self.state.lock().await;
        if guard.legacy_pb {
            let res: UdpRes = packet
                .try_into()
                .map_err(|_| io::Error::other("UdpRes conversion failed"))?;
            let encoded = res.encode_to_vec();
            guard.send.send_lpm_payload(&encoded).await
        } else {
            let frame = encode_udp_packet_frame(
                &packet.meta.dst_addr,
                packet.meta.dst_port,
                &mut guard.last_dst,
                &packet.data,
            )?;
            guard.send.send_raw_bytes(frame).await
        }
    }
}

pub struct H3UdpServerReadHalf {
    recv: ServerRecvBody,
    decoder: LpmDecoder,
    expected_auth: String,
    src_endpoint: Option<(String, u16)>,
    last_dst: Option<(String, u16)>,
    legacy_pb: bool,
}

impl H3UdpServerReadHalf {
    pub fn new(recv: ServerRecvBody, expected_auth: String, legacy_pb: bool) -> Self {
        Self {
            recv,
            decoder: LpmDecoder::new(),
            expected_auth,
            src_endpoint: None,
            last_dst: None,
            legacy_pb,
        }
    }

    async fn read_next_lpm_frame(&mut self) -> io::Result<Bytes> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(frame);
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
                        "H3 UDP server stream closed",
                    ));
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl RunUdpReader for H3UdpServerReadHalf {
    async fn read(&mut self) -> io::Result<UDPPacket> {
        if self.legacy_pb {
            let frame = self.read_next_lpm_frame().await?;
            let req = UdpReq::decode(frame).map_err(|e| {
                io::Error::new(ErrorKind::InvalidData, format!("UdpReq decode: {e}"))
            })?;
            if req.auth != self.expected_auth {
                return Err(io::Error::new(
                    ErrorKind::PermissionDenied,
                    "invalid legacy UDP auth",
                ));
            }
            return req
                .try_into()
                .map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid UdpReq fields"));
        }

        if self.src_endpoint.is_none() {
            let hs_frame = self.read_next_lpm_frame().await?;
            let (auth, src_host, src_port) = decode_udp_handshake_frame(hs_frame)?;
            if auth != self.expected_auth {
                return Err(io::Error::new(
                    ErrorKind::PermissionDenied,
                    "invalid H3 UDP auth",
                ));
            }
            self.src_endpoint = Some((src_host, src_port));
        }
        let (src_host, src_port) = self.src_endpoint.as_ref().unwrap().clone();
        let pkt_frame = self.read_next_lpm_frame().await?;
        decode_udp_packet_frame(pkt_frame, &mut self.last_dst, &src_host, src_port)
    }
}
