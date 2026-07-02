use bytes::{Buf, Bytes, BytesMut};
use chacha20poly1305::aead::{OsRng, rand_core::RngCore};
use h2::{RecvStream, SendStream};
use h3::client::RequestStream as H3RequestStream;
use prost::Message;
use std::future::poll_fn;
use std::io;
use std::time::Duration;

pub const CONTENT_TYPE_PROTOBUF: &str = "application/x-protobuf";
pub const DEFAULT_PATH: &str = "pb_http";

pub type H3ClientSendStream = H3RequestStream<h3_quinn::SendStream<Bytes>, Bytes>;
pub type H3ClientRecvStream = H3RequestStream<h3_quinn::RecvStream, Bytes>;
pub type H3ClientRequestSender = h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>;

const DEFAULT_MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;
const DEFAULT_INITIAL_STREAM_WINDOW_SIZE: u32 = 1024 * 1024;
const DEFAULT_INITIAL_CONNECTION_WINDOW_SIZE: u32 = 16 * 1024 * 1024;
const DEFAULT_MAX_FRAME_SIZE: u32 = 64 * 1024;
const DEFAULT_MAX_HEADER_LIST_SIZE: u32 = 16 * 1024;
const DEFAULT_MAX_CONCURRENT_STREAMS: u32 = 1024;
const DEFAULT_MAX_SEND_BUFFER_SIZE: usize = 1024 * 1024;
const DEFAULT_ACCEPT_CHANNEL_SIZE: usize = 64;
const DEFAULT_SEND_CHUNK_SIZE: usize = 64 * 1024;
const DEFAULT_H3_MAX_FIELD_SECTION_SIZE: u64 = 16 * 1024;
const DEFAULT_QUIC_MAX_IDLE_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_QUIC_KEEP_ALIVE_INTERVAL_MS: u64 = 15_000;
const DEFAULT_QUIC_INITIAL_RTT_MS: u64 = 100;
const DEFAULT_QUIC_STREAM_RECEIVE_WINDOW: u64 = 1024 * 1024;
const DEFAULT_QUIC_RECEIVE_WINDOW: u64 = 16 * 1024 * 1024;
const DEFAULT_QUIC_SEND_WINDOW: u64 = 16 * 1024 * 1024;
const DEFAULT_QUIC_MAX_CONCURRENT_BIDI_STREAMS: u64 = 1024;
const DEFAULT_QUIC_MAX_CONCURRENT_UNI_STREAMS: u64 = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PbHttpTransport {
    H2c,
    H2,
    H3,
}

impl PbHttpTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::H2c => "h2c",
            Self::H2 => "h2",
            Self::H3 => "h3",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PbHttpOptions {
    pub path: String,
    pub client_transports: Vec<PbHttpTransport>,
    pub tls_server_name: Option<String>,
    pub tls_insecure_skip_verify: bool,
    pub tls_ca_cert_file: Option<String>,
    pub max_message_size: usize,
    pub initial_stream_window_size: u32,
    pub initial_connection_window_size: u32,
    pub max_frame_size: u32,
    pub max_header_list_size: u32,
    pub max_concurrent_streams: u32,
    pub header_table_size: Option<u32>,
    pub max_send_buffer_size: usize,
    pub max_concurrent_reset_streams: Option<usize>,
    pub max_pending_accept_reset_streams: Option<usize>,
    pub reset_stream_duration: Option<Duration>,
    pub accept_channel_size: usize,
    pub send_chunk_size: usize,
    pub h3_max_field_section_size: u64,
    pub h3_send_grease: bool,
    pub h3_bind: String,
    pub quic_max_idle_timeout: Duration,
    pub quic_keep_alive_interval: Option<Duration>,
    pub quic_initial_rtt: Duration,
    pub quic_stream_receive_window: u64,
    pub quic_receive_window: u64,
    pub quic_send_window: u64,
    pub quic_max_concurrent_bidi_streams: u64,
    pub quic_max_concurrent_uni_streams: u64,
    pub quic_send_fairness: bool,
}

impl Default for PbHttpOptions {
    fn default() -> Self {
        Self {
            path: DEFAULT_PATH.to_string(),
            client_transports: vec![PbHttpTransport::H3, PbHttpTransport::H2],
            tls_server_name: None,
            tls_insecure_skip_verify: false,
            tls_ca_cert_file: None,
            max_message_size: DEFAULT_MAX_MESSAGE_SIZE,
            initial_stream_window_size: DEFAULT_INITIAL_STREAM_WINDOW_SIZE,
            initial_connection_window_size: DEFAULT_INITIAL_CONNECTION_WINDOW_SIZE,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            max_header_list_size: DEFAULT_MAX_HEADER_LIST_SIZE,
            max_concurrent_streams: DEFAULT_MAX_CONCURRENT_STREAMS,
            header_table_size: None,
            max_send_buffer_size: DEFAULT_MAX_SEND_BUFFER_SIZE,
            max_concurrent_reset_streams: None,
            max_pending_accept_reset_streams: None,
            reset_stream_duration: None,
            accept_channel_size: DEFAULT_ACCEPT_CHANNEL_SIZE,
            send_chunk_size: DEFAULT_SEND_CHUNK_SIZE,
            h3_max_field_section_size: DEFAULT_H3_MAX_FIELD_SECTION_SIZE,
            h3_send_grease: true,
            h3_bind: "0.0.0.0:0".to_string(),
            quic_max_idle_timeout: Duration::from_millis(DEFAULT_QUIC_MAX_IDLE_TIMEOUT_MS),
            quic_keep_alive_interval: Some(Duration::from_millis(
                DEFAULT_QUIC_KEEP_ALIVE_INTERVAL_MS,
            )),
            quic_initial_rtt: Duration::from_millis(DEFAULT_QUIC_INITIAL_RTT_MS),
            quic_stream_receive_window: DEFAULT_QUIC_STREAM_RECEIVE_WINDOW,
            quic_receive_window: DEFAULT_QUIC_RECEIVE_WINDOW,
            quic_send_window: DEFAULT_QUIC_SEND_WINDOW,
            quic_max_concurrent_bidi_streams: DEFAULT_QUIC_MAX_CONCURRENT_BIDI_STREAMS,
            quic_max_concurrent_uni_streams: DEFAULT_QUIC_MAX_CONCURRENT_UNI_STREAMS,
            quic_send_fairness: true,
        }
    }
}

impl PbHttpOptions {
    pub fn from_options(
        options: &Option<std::collections::HashMap<String, toml::Value>>,
    ) -> io::Result<Self> {
        let mut cfg = Self::default();
        cfg.path = normalize_route_path(
            &option_string(options, "path", cfg.path.clone())?,
            "pb_http path",
        )?;
        if let Some(transport) = option_string_opt(options, "transport")? {
            cfg.client_transports = vec![parse_transport(&transport)?];
        } else if let Some(order) = option_string_list_opt(options, "fallback_order")? {
            if order.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "pb_http fallback_order must not be empty",
                ));
            }
            cfg.client_transports = order
                .iter()
                .map(|v| parse_transport(v))
                .collect::<io::Result<Vec<_>>>()?;
        }
        cfg.tls_server_name = option_string_opt(options, "tls_server_name")?;
        cfg.tls_insecure_skip_verify = option_bool(
            options,
            "tls_insecure_skip_verify",
            cfg.tls_insecure_skip_verify,
        )?;
        cfg.tls_ca_cert_file = option_string_opt(options, "tls_ca_cert_file")?;
        cfg.max_message_size = option_usize(options, "max_message_size", cfg.max_message_size)?;
        cfg.initial_stream_window_size = option_u32(
            options,
            "h2_initial_stream_window_size",
            cfg.initial_stream_window_size,
        )?;
        cfg.initial_connection_window_size = option_u32(
            options,
            "h2_initial_connection_window_size",
            cfg.initial_connection_window_size,
        )?;
        cfg.max_frame_size = option_u32(options, "h2_max_frame_size", cfg.max_frame_size)?;
        cfg.max_header_list_size =
            option_u32(options, "h2_max_header_list_size", cfg.max_header_list_size)?;
        cfg.max_concurrent_streams = option_u32(
            options,
            "h2_max_concurrent_streams",
            cfg.max_concurrent_streams,
        )?;
        cfg.header_table_size = option_u32_opt(options, "h2_header_table_size")?;
        cfg.max_send_buffer_size =
            option_usize(options, "h2_max_send_buffer_size", cfg.max_send_buffer_size)?;
        cfg.max_concurrent_reset_streams =
            option_usize_opt(options, "h2_max_concurrent_reset_streams")?;
        cfg.max_pending_accept_reset_streams =
            option_usize_opt(options, "h2_max_pending_accept_reset_streams")?;
        cfg.reset_stream_duration = option_duration_opt(options, "h2_reset_stream_duration_ms")?;
        cfg.accept_channel_size =
            option_usize(options, "accept_channel_size", cfg.accept_channel_size)?.max(1);
        cfg.send_chunk_size = option_usize(options, "send_chunk_size", cfg.send_chunk_size)?.max(1);
        cfg.h3_max_field_section_size = option_u64(
            options,
            "h3_max_field_section_size",
            cfg.h3_max_field_section_size,
        )?;
        cfg.h3_send_grease = option_bool(options, "h3_send_grease", cfg.h3_send_grease)?;
        cfg.h3_bind = option_string(options, "h3_bind", cfg.h3_bind)?;
        cfg.quic_max_idle_timeout = Duration::from_millis(option_u64(
            options,
            "quic_max_idle_timeout_ms",
            cfg.quic_max_idle_timeout.as_millis() as u64,
        )?);
        let keep_alive = option_u64_opt(options, "quic_keep_alive_interval_ms")?;
        cfg.quic_keep_alive_interval = match keep_alive {
            Some(0) => None,
            Some(v) => Some(Duration::from_millis(v)),
            None => cfg.quic_keep_alive_interval,
        };
        cfg.quic_initial_rtt = Duration::from_millis(option_u64(
            options,
            "quic_initial_rtt_ms",
            cfg.quic_initial_rtt.as_millis() as u64,
        )?);
        cfg.quic_stream_receive_window = option_u64(
            options,
            "quic_stream_receive_window",
            cfg.quic_stream_receive_window,
        )?;
        cfg.quic_receive_window =
            option_u64(options, "quic_receive_window", cfg.quic_receive_window)?;
        cfg.quic_send_window = option_u64(options, "quic_send_window", cfg.quic_send_window)?;
        cfg.quic_max_concurrent_bidi_streams = option_u64(
            options,
            "quic_max_concurrent_bidi_streams",
            cfg.quic_max_concurrent_bidi_streams,
        )?;
        cfg.quic_max_concurrent_uni_streams = option_u64(
            options,
            "quic_max_concurrent_uni_streams",
            cfg.quic_max_concurrent_uni_streams,
        )?;
        cfg.quic_send_fairness =
            option_bool(options, "quic_send_fairness", cfg.quic_send_fairness)?;

        if cfg.max_message_size == 0 || cfg.max_message_size > DEFAULT_MAX_MESSAGE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http max_message_size must be between 1 and 16777216",
            ));
        }
        if !(16_384..=16_777_215).contains(&cfg.max_frame_size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http h2_max_frame_size must be between 16384 and 16777215",
            ));
        }
        if cfg.max_send_buffer_size > u32::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http h2_max_send_buffer_size must be <= u32::MAX",
            ));
        }
        if cfg.h3_max_field_section_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http h3_max_field_section_size must be > 0",
            ));
        }
        if cfg.quic_max_idle_timeout.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http quic_max_idle_timeout_ms must be > 0",
            ));
        }
        if cfg.quic_initial_rtt.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http quic_initial_rtt_ms must be > 0",
            ));
        }
        Ok(cfg)
    }

    pub fn stream_path(&self) -> String {
        format!("/{}/stream", self.path)
    }

    pub fn udp_path(&self) -> String {
        format!("/{}/udp", self.path)
    }

    pub fn apply_client_builder(&self, builder: &mut h2::client::Builder) {
        builder
            .initial_window_size(self.initial_stream_window_size)
            .initial_connection_window_size(self.initial_connection_window_size)
            .max_frame_size(self.max_frame_size)
            .max_header_list_size(self.max_header_list_size)
            .max_concurrent_streams(self.max_concurrent_streams)
            .max_send_buffer_size(self.max_send_buffer_size)
            .enable_push(false);
        if let Some(v) = self.header_table_size {
            builder.header_table_size(v);
        }
        if let Some(v) = self.max_concurrent_reset_streams {
            builder.max_concurrent_reset_streams(v);
        }
        if let Some(v) = self.max_pending_accept_reset_streams {
            builder.max_pending_accept_reset_streams(v);
        }
        if let Some(v) = self.reset_stream_duration {
            builder.reset_stream_duration(v);
        }
    }

    pub fn apply_server_builder(&self, builder: &mut h2::server::Builder) {
        builder
            .initial_window_size(self.initial_stream_window_size)
            .initial_connection_window_size(self.initial_connection_window_size)
            .max_frame_size(self.max_frame_size)
            .max_header_list_size(self.max_header_list_size)
            .max_concurrent_streams(self.max_concurrent_streams)
            .max_send_buffer_size(self.max_send_buffer_size);
        if let Some(v) = self.max_concurrent_reset_streams {
            builder.max_concurrent_reset_streams(v);
        }
        if let Some(v) = self.max_pending_accept_reset_streams {
            builder.max_pending_accept_reset_streams(v);
        }
        if let Some(v) = self.reset_stream_duration {
            builder.reset_stream_duration(v);
        }
    }

    pub fn apply_h3_client_builder(&self, builder: &mut h3::client::Builder) {
        builder
            .max_field_section_size(self.h3_max_field_section_size)
            .send_grease(self.h3_send_grease);
    }

    pub fn quic_transport_config(&self) -> io::Result<quinn::TransportConfig> {
        let mut transport = quinn::TransportConfig::default();
        transport
            .max_idle_timeout(Some(
                self.quic_max_idle_timeout
                    .try_into()
                    .map_err(io::Error::other)?,
            ))
            .keep_alive_interval(self.quic_keep_alive_interval)
            .initial_rtt(self.quic_initial_rtt)
            .stream_receive_window(varint(
                self.quic_stream_receive_window,
                "quic_stream_receive_window",
            )?)
            .receive_window(varint(self.quic_receive_window, "quic_receive_window")?)
            .send_window(self.quic_send_window)
            .max_concurrent_bidi_streams(varint(
                self.quic_max_concurrent_bidi_streams,
                "quic_max_concurrent_bidi_streams",
            )?)
            .max_concurrent_uni_streams(varint(
                self.quic_max_concurrent_uni_streams,
                "quic_max_concurrent_uni_streams",
            )?)
            .send_fairness(self.quic_send_fairness);
        Ok(transport)
    }
}

pub async fn read_message<M: Message + Default>(
    reader: &mut RecvStream,
    buf: &mut BytesMut,
    max_message_size: usize,
) -> io::Result<Option<M>> {
    while buf.len() < 16 {
        if !read_data(reader, buf).await? {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "pb_http frame header truncated",
                ))
            };
        }
    }

    let r = u64::from_be_bytes(buf[..8].try_into().unwrap());
    let second = u64::from_be_bytes(buf[8..16].try_into().unwrap());
    let len = r.wrapping_add(second) as usize;
    if len > max_message_size {
        return Err(io::Error::other(format!(
            "pb_http frame too large: {} > {}",
            len, max_message_size
        )));
    }

    while buf.len() < 16 + len {
        if !read_data(reader, buf).await? {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "pb_http frame body truncated",
            ));
        }
    }

    buf.advance(16);
    let data = buf.split_to(len);
    M::decode(data.as_ref())
        .map(Some)
        .map_err(|e| io::Error::other(format!("protobuf decode: {}", e)))
}

pub async fn send_message<M: Message>(
    writer: &mut SendStream<Bytes>,
    msg: &M,
    send_chunk_size: usize,
) -> io::Result<()> {
    send_bytes(writer, encode_message_frame(msg), send_chunk_size).await
}

pub async fn read_h3_message<M: Message + Default>(
    reader: &mut H3ClientRecvStream,
    buf: &mut BytesMut,
    max_message_size: usize,
) -> io::Result<Option<M>> {
    while buf.len() < 16 {
        if !read_h3_data(reader, buf).await? {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "pb_http h3 frame header truncated",
                ))
            };
        }
    }

    let r = u64::from_be_bytes(buf[..8].try_into().unwrap());
    let second = u64::from_be_bytes(buf[8..16].try_into().unwrap());
    let len = r.wrapping_add(second) as usize;
    if len > max_message_size {
        return Err(io::Error::other(format!(
            "pb_http h3 frame too large: {} > {}",
            len, max_message_size
        )));
    }

    while buf.len() < 16 + len {
        if !read_h3_data(reader, buf).await? {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "pb_http h3 frame body truncated",
            ));
        }
    }

    buf.advance(16);
    let data = buf.split_to(len);
    M::decode(data.as_ref())
        .map(Some)
        .map_err(|e| io::Error::other(format!("protobuf decode: {}", e)))
}

pub async fn send_h3_message<M: Message>(
    writer: &mut H3ClientSendStream,
    msg: &M,
    send_chunk_size: usize,
) -> io::Result<()> {
    send_h3_bytes(writer, encode_message_frame(msg), send_chunk_size).await
}

fn encode_message_frame<M: Message>(msg: &M) -> Bytes {
    let data = msg.encode_to_vec();
    let mut frame = BytesMut::with_capacity(16 + data.len());
    let len = data.len() as u64;
    let r = OsRng.next_u64();
    let second = len.wrapping_sub(r);
    frame.extend_from_slice(&r.to_be_bytes());
    frame.extend_from_slice(&second.to_be_bytes());
    frame.extend_from_slice(&data);
    frame.freeze()
}

async fn read_data(reader: &mut RecvStream, buf: &mut BytesMut) -> io::Result<bool> {
    loop {
        match reader.data().await {
            None => return Ok(false),
            Some(Err(e)) => return Err(io::Error::other(e)),
            Some(Ok(chunk)) => {
                let len = chunk.len();
                if len == 0 {
                    continue;
                }
                buf.extend_from_slice(&chunk);
                reader
                    .flow_control()
                    .release_capacity(len)
                    .map_err(io::Error::other)?;
                return Ok(true);
            }
        }
    }
}

async fn read_h3_data(reader: &mut H3ClientRecvStream, buf: &mut BytesMut) -> io::Result<bool> {
    loop {
        match reader.recv_data().await {
            Ok(None) => return Ok(false),
            Err(e) => return Err(io::Error::other(e)),
            Ok(Some(mut chunk)) => {
                if !chunk.has_remaining() {
                    continue;
                }
                while chunk.has_remaining() {
                    let len = chunk.remaining();
                    let bytes = chunk.copy_to_bytes(len);
                    buf.extend_from_slice(&bytes);
                }
                return Ok(true);
            }
        }
    }
}

async fn send_bytes(
    writer: &mut SendStream<Bytes>,
    mut data: Bytes,
    send_chunk_size: usize,
) -> io::Result<()> {
    while data.has_remaining() {
        let requested = data.remaining().min(send_chunk_size);
        writer.reserve_capacity(requested);
        let capacity = poll_fn(|cx| writer.poll_capacity(cx))
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "pb_http stream closed"))?
            .map_err(io::Error::other)?;
        if capacity == 0 {
            continue;
        }
        let n = requested.min(capacity);
        let chunk = data.split_to(n);
        writer.send_data(chunk, false).map_err(io::Error::other)?;
    }
    Ok(())
}

async fn send_h3_bytes(
    writer: &mut H3ClientSendStream,
    mut data: Bytes,
    send_chunk_size: usize,
) -> io::Result<()> {
    while data.has_remaining() {
        let n = data.remaining().min(send_chunk_size);
        let chunk = data.split_to(n);
        writer.send_data(chunk).await.map_err(io::Error::other)?;
    }
    Ok(())
}

pub fn request_uri(scheme: &str, authority: &str, path: &str) -> String {
    format!("{}://{}{}", scheme, authority, path)
}

fn option_value<'a>(
    options: &'a Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
) -> Option<&'a toml::Value> {
    options.as_ref().and_then(|m| m.get(key))
}

fn option_u32(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
    default: u32,
) -> io::Result<u32> {
    Ok(option_u32_opt(options, key)?.unwrap_or(default))
}

fn option_u64(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
    default: u64,
) -> io::Result<u64> {
    Ok(option_u64_opt(options, key)?.unwrap_or(default))
}

fn option_u64_opt(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
) -> io::Result<Option<u64>> {
    match option_value(options, key) {
        None => Ok(None),
        Some(v) => value_u64(v, key).map(Some),
    }
}

fn option_u32_opt(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
) -> io::Result<Option<u32>> {
    match option_value(options, key) {
        None => Ok(None),
        Some(v) => value_u64(v, key).and_then(|n| {
            u32::try_from(n).map(Some).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, format!("{} is too large", key))
            })
        }),
    }
}

fn option_usize(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
    default: usize,
) -> io::Result<usize> {
    Ok(option_usize_opt(options, key)?.unwrap_or(default))
}

fn option_usize_opt(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
) -> io::Result<Option<usize>> {
    match option_value(options, key) {
        None => Ok(None),
        Some(v) => value_u64(v, key).and_then(|n| {
            usize::try_from(n).map(Some).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, format!("{} is too large", key))
            })
        }),
    }
}

fn option_duration_opt(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
) -> io::Result<Option<Duration>> {
    match option_value(options, key) {
        None => Ok(None),
        Some(v) => Ok(Some(Duration::from_millis(value_u64(v, key)?))),
    }
}

fn option_bool(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
    default: bool,
) -> io::Result<bool> {
    match option_value(options, key) {
        None => Ok(default),
        Some(toml::Value::Boolean(v)) => Ok(*v),
        Some(toml::Value::String(v)) => match v.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} must be a boolean", key),
            )),
        },
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} must be a boolean", key),
        )),
    }
}

fn option_string(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
    default: String,
) -> io::Result<String> {
    Ok(option_string_opt(options, key)?.unwrap_or(default))
}

fn option_string_opt(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
) -> io::Result<Option<String>> {
    match option_value(options, key) {
        None => Ok(None),
        Some(toml::Value::String(v)) => Ok(Some(v.clone())),
        Some(v) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} must be a string, got {}", key, v.type_str()),
        )),
    }
}

fn option_string_list_opt(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    key: &str,
) -> io::Result<Option<Vec<String>>> {
    match option_value(options, key) {
        None => Ok(None),
        Some(toml::Value::String(v)) => Ok(Some(
            v.split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        )),
        Some(toml::Value::Array(values)) => values
            .iter()
            .map(|v| match v {
                toml::Value::String(s) => Ok(s.clone()),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} must contain only strings", key),
                )),
            })
            .collect::<io::Result<Vec<_>>>()
            .map(Some),
        Some(v) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} must be a string or string array, got {}",
                key,
                v.type_str()
            ),
        )),
    }
}

fn value_u64(v: &toml::Value, key: &str) -> io::Result<u64> {
    match v {
        toml::Value::Integer(i) if *i >= 0 => Ok(*i as u64),
        toml::Value::String(s) => parse_size_or_number(s, key),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} must be a non-negative integer or size string", key),
        )),
    }
}

fn normalize_route_path(path: &str, key: &str) -> io::Result<String> {
    let path = path.trim().trim_matches('/');
    if path.is_empty()
        || path.contains('?')
        || path.contains('#')
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} must be a non-empty relative HTTP path", key),
        ));
    }
    Ok(path.to_string())
}

fn parse_transport(v: &str) -> io::Result<PbHttpTransport> {
    match v.trim().to_ascii_lowercase().as_str() {
        "h2c" => Ok(PbHttpTransport::H2c),
        "h2" | "http2" | "http/2" => Ok(PbHttpTransport::H2),
        "h3" | "http3" | "http/3" => Ok(PbHttpTransport::H3),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported pb_http transport {:?}", v),
        )),
    }
}

fn varint(value: u64, key: &str) -> io::Result<quinn::VarInt> {
    quinn::VarInt::from_u64(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is too large for QUIC varint", key),
        )
    })
}

fn parse_size_or_number(s: &str, key: &str) -> io::Result<u64> {
    crate::util::parse::parse_size(s).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid {} value {:?}: {}", key, s, e),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::v1::pb::{StreamReq, StreamRes};
    use http::{Method, Request, Response, StatusCode};
    use tokio::io::duplex;

    #[tokio::test]
    async fn h2c_protobuf_frame_round_trip() {
        let (client_io, server_io) = duplex(1024 * 1024);
        let (finish_tx, mut finish_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut conn = h2::server::handshake(server_io).await.unwrap();
            let (req, respond) = conn.accept().await.unwrap().unwrap();
            tokio::spawn(async move {
                let mut respond = respond;
                assert_eq!(req.method(), Method::POST);
                let path = PbHttpOptions::default().stream_path();
                assert_eq!(req.uri().path(), path);
                let mut writer = respond
                    .send_response(
                        Response::builder().status(StatusCode::OK).body(()).unwrap(),
                        false,
                    )
                    .unwrap();
                let mut body = req.into_body();
                let mut frame_buf = BytesMut::new();
                let msg: StreamReq = read_message(&mut body, &mut frame_buf, 1024)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(msg.auth, "auth");
                assert_eq!(msg.dst_addr.as_deref(), Some("example.com"));
                send_message(
                    &mut writer,
                    &StreamRes {
                        payload: b"pong".to_vec(),
                    },
                    1024,
                )
                .await
                .unwrap();
            });
            loop {
                tokio::select! {
                    _ = &mut finish_rx => break,
                    accepted = conn.accept() => {
                        if accepted.is_none() {
                            break;
                        }
                    }
                }
            }
        });

        let (mut client, connection) = h2::client::handshake(client_io).await.unwrap();
        tokio::spawn(async move {
            connection.await.unwrap();
        });
        let request = Request::builder()
            .method(Method::POST)
            .uri(request_uri(
                "http",
                "localhost",
                &PbHttpOptions::default().stream_path(),
            ))
            .body(())
            .unwrap();
        let (response, mut writer) = client.send_request(request, false).unwrap();
        send_message(
            &mut writer,
            &StreamReq {
                auth: "auth".to_string(),
                dst_addr: Some("example.com".to_string()),
                dst_port: Some(443),
                ..Default::default()
            },
            1024,
        )
        .await
        .unwrap();

        let response = response.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let mut frame_buf = BytesMut::new();
        let msg: StreamRes = read_message(&mut body, &mut frame_buf, 1024)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(msg.payload, b"pong");
        let _ = finish_tx.send(());
        server.await.unwrap();
    }

    #[test]
    fn parses_route_path_and_fallback_order() {
        let options = Some(std::collections::HashMap::from([
            (
                "path".to_string(),
                toml::Value::String("/tenant-a/pb_http/".to_string()),
            ),
            (
                "fallback_order".to_string(),
                toml::Value::Array(vec![
                    toml::Value::String("h2".to_string()),
                    toml::Value::String("h3".to_string()),
                ]),
            ),
        ]));
        let cfg = PbHttpOptions::from_options(&options).unwrap();
        assert_eq!(cfg.stream_path(), "/tenant-a/pb_http/stream");
        assert_eq!(cfg.udp_path(), "/tenant-a/pb_http/udp");
        assert_eq!(
            cfg.client_transports,
            vec![PbHttpTransport::H2, PbHttpTransport::H3]
        );
    }
}
