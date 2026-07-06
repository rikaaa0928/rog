use hyper_util::rt::TokioIo;
use socket2::{SockRef, TcpKeepalive};
use std::io;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;
use tonic::transport::Server;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

use crate::util::parse::parse_size;

#[derive(Debug, Clone)]
pub(crate) struct GrpcTransportOptions {
    pub keep_alive: bool,
    pub keep_alive_interval: Duration,
    pub keep_alive_timeout: Duration,
    pub keep_alive_while_idle: bool,
    pub connect_timeout: Option<Duration>,
    pub tcp_keepalive: Option<Duration>,
    pub tcp_keepalive_interval: Option<Duration>,
    pub tcp_keepalive_retries: Option<u32>,
    pub tcp_nodelay: bool,
    pub initial_stream_window_size: Option<u32>,
    pub initial_connection_window_size: Option<u32>,
    pub http2_adaptive_window: Option<bool>,
    pub http2_max_header_list_size: Option<u32>,
    pub max_frame_size: Option<u32>,
    pub max_concurrent_streams: Option<u32>,
    pub concurrency_limit: Option<usize>,
    pub concurrency_limit_per_connection: Option<usize>,
    pub channel_buffer_size: Option<usize>,
    pub stream_channel_size: Option<usize>,
    pub max_decoding_message_size: Option<usize>,
    pub max_encoding_message_size: Option<usize>,
}

impl GrpcTransportOptions {
    pub fn from_options(options: &Option<std::collections::HashMap<String, toml::Value>>) -> Self {
        let keep_alive = option_bool(options, &["keep_alive"], false);
        let keep_alive_interval = option_duration_secs(
            options,
            &["keep_alive_interval_secs", "http2_keepalive_interval_secs"],
            30,
        );
        let keep_alive_timeout = option_duration_secs(
            options,
            &["keep_alive_timeout_secs", "http2_keepalive_timeout_secs"],
            20,
        );
        let max_message_size =
            option_size(options, &["max_message_size", "max_message_size_bytes"]);

        Self {
            keep_alive,
            keep_alive_interval,
            keep_alive_timeout,
            keep_alive_while_idle: option_bool(options, &["keep_alive_while_idle"], true),
            connect_timeout: option_duration_secs_opt(options, &["connect_timeout_secs"]),
            tcp_keepalive: option_duration_secs_opt(options, &["tcp_keepalive_secs"]),
            tcp_keepalive_interval: option_duration_secs_opt(
                options,
                &["tcp_keepalive_interval_secs"],
            ),
            tcp_keepalive_retries: option_u32(options, &["tcp_keepalive_retries"]),
            tcp_nodelay: option_bool(options, &["tcp_nodelay"], true),
            initial_stream_window_size: option_size_u32(
                options,
                &[
                    "initial_stream_window_size",
                    "http2_initial_stream_window_size",
                ],
            ),
            initial_connection_window_size: option_size_u32(
                options,
                &[
                    "initial_connection_window_size",
                    "http2_initial_connection_window_size",
                ],
            ),
            http2_adaptive_window: option_bool_opt(options, &["http2_adaptive_window"]),
            http2_max_header_list_size: option_size_u32(options, &["http2_max_header_list_size"]),
            max_frame_size: option_size_u32(options, &["max_frame_size", "http2_max_frame_size"]),
            max_concurrent_streams: option_u32(options, &["max_concurrent_streams"]),
            concurrency_limit: option_usize(options, &["concurrency_limit"]),
            concurrency_limit_per_connection: option_usize(
                options,
                &["concurrency_limit_per_connection"],
            ),
            channel_buffer_size: option_usize(options, &["channel_buffer_size"]),
            stream_channel_size: option_usize(options, &["stream_channel_size"]),
            max_decoding_message_size: option_size(
                options,
                &[
                    "max_decoding_message_size",
                    "max_decoding_message_size_bytes",
                ],
            )
            .or(max_message_size),
            max_encoding_message_size: option_size(
                options,
                &[
                    "max_encoding_message_size",
                    "max_encoding_message_size_bytes",
                ],
            )
            .or(max_message_size),
        }
    }

    pub fn stream_channel_size_or(&self, default: usize) -> usize {
        self.stream_channel_size.unwrap_or(default).max(1)
    }
}

pub(crate) fn configure_endpoint(
    mut endpoint: Endpoint,
    options: &GrpcTransportOptions,
) -> Endpoint {
    if options.keep_alive {
        endpoint = endpoint
            .http2_keep_alive_interval(options.keep_alive_interval)
            .keep_alive_timeout(options.keep_alive_timeout)
            .keep_alive_while_idle(options.keep_alive_while_idle);
    }
    if let Some(timeout) = options.connect_timeout {
        endpoint = endpoint.connect_timeout(timeout);
    }
    if let Some(tcp_keepalive) = options.tcp_keepalive {
        endpoint = endpoint.tcp_keepalive(Some(tcp_keepalive));
    }
    if let Some(tcp_keepalive_interval) = options.tcp_keepalive_interval {
        endpoint = endpoint.tcp_keepalive_interval(Some(tcp_keepalive_interval));
    }
    if let Some(tcp_keepalive_retries) = options.tcp_keepalive_retries {
        endpoint = endpoint.tcp_keepalive_retries(Some(tcp_keepalive_retries));
    }
    endpoint = endpoint.tcp_nodelay(options.tcp_nodelay);
    if let Some(size) = options.initial_stream_window_size {
        endpoint = endpoint.initial_stream_window_size(size);
    }
    if let Some(size) = options.initial_connection_window_size {
        endpoint = endpoint.initial_connection_window_size(size);
    }
    if let Some(enabled) = options.http2_adaptive_window {
        endpoint = endpoint.http2_adaptive_window(enabled);
    }
    if let Some(size) = options.http2_max_header_list_size {
        endpoint = endpoint.http2_max_header_list_size(size);
    }
    if let Some(limit) = options.concurrency_limit {
        endpoint = endpoint.concurrency_limit(limit);
    }
    if let Some(size) = options.channel_buffer_size {
        endpoint = endpoint.buffer_size(size);
    }
    endpoint
}

pub(crate) fn configure_server(mut server: Server, options: &GrpcTransportOptions) -> Server {
    if options.keep_alive {
        server = server
            .http2_keepalive_interval(Some(options.keep_alive_interval))
            .http2_keepalive_timeout(Some(options.keep_alive_timeout));
    }
    if let Some(tcp_keepalive) = options.tcp_keepalive {
        server = server.tcp_keepalive(Some(tcp_keepalive));
    }
    server = server.tcp_nodelay(options.tcp_nodelay);
    if let Some(size) = options.initial_stream_window_size {
        server = server.initial_stream_window_size(size);
    }
    if let Some(size) = options.initial_connection_window_size {
        server = server.initial_connection_window_size(size);
    }
    if let Some(max) = options.max_concurrent_streams {
        server = server.max_concurrent_streams(max);
    }
    if let Some(enabled) = options.http2_adaptive_window {
        server = server.http2_adaptive_window(Some(enabled));
    }
    if let Some(size) = options.http2_max_header_list_size {
        server = server.http2_max_header_list_size(size);
    }
    if let Some(size) = options.max_frame_size {
        server = server.max_frame_size(size);
    }
    if let Some(limit) = options.concurrency_limit_per_connection {
        server = server.concurrency_limit_per_connection(limit);
    }
    server
}

fn default_port(uri: &Uri) -> u16 {
    match uri.scheme_str() {
        Some("https") => 443,
        _ => 80,
    }
}

pub(crate) async fn connect_channel_without_proxy(
    endpoint: Endpoint,
) -> Result<Channel, tonic::transport::Error> {
    let tcp_nodelay = endpoint.get_tcp_nodelay();
    let connect_timeout = endpoint.get_connect_timeout();
    let tcp_keepalive = endpoint.get_tcp_keepalive();
    let tcp_keepalive_interval = endpoint.get_tcp_keepalive_interval();
    let tcp_keepalive_retries = endpoint.get_tcp_keepalive_retries();

    endpoint
        .connect_with_connector(service_fn(move |uri: Uri| {
            let tcp_nodelay = tcp_nodelay;
            let connect_timeout = connect_timeout;
            let tcp_keepalive = tcp_keepalive;
            let tcp_keepalive_interval = tcp_keepalive_interval;
            let tcp_keepalive_retries = tcp_keepalive_retries;

            async move {
                let host = uri.host().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "grpc endpoint missing host")
                })?;
                let port = uri.port_u16().unwrap_or_else(|| default_port(&uri));
                let connect = TcpStream::connect((host, port));
                let stream = match connect_timeout {
                    Some(duration) => timeout(duration, connect).await.map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "grpc connect timeout")
                    })??,
                    None => connect.await?,
                };

                stream.set_nodelay(tcp_nodelay)?;
                if let Some(tcp_keepalive) = tcp_keepalive {
                    apply_tcp_keepalive(
                        &stream,
                        tcp_keepalive,
                        tcp_keepalive_interval,
                        tcp_keepalive_retries,
                    )?;
                }

                Ok::<_, io::Error>(TokioIo::new(stream))
            }
        }))
        .await
}

fn apply_tcp_keepalive(
    stream: &TcpStream,
    time: Duration,
    interval: Option<Duration>,
    retries: Option<u32>,
) -> io::Result<()> {
    let mut keepalive = TcpKeepalive::new().with_time(time);

    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "ios",
        target_os = "visionos",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "windows",
        target_os = "cygwin",
        all(target_os = "wasi", not(target_env = "p1")),
    ))]
    {
        if let Some(interval) = interval {
            keepalive = keepalive.with_interval(interval);
        }
        if let Some(retries) = retries {
            keepalive = keepalive.with_retries(retries);
        }
    }

    SockRef::from(stream).set_tcp_keepalive(&keepalive)
}

fn option_value<'a>(
    options: &'a Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
) -> Option<&'a toml::Value> {
    let options = options.as_ref()?;
    keys.iter().find_map(|key| options.get(*key))
}

fn option_bool(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
    default: bool,
) -> bool {
    option_bool_opt(options, keys).unwrap_or(default)
}

fn option_bool_opt(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
) -> Option<bool> {
    option_value(options, keys).and_then(toml::Value::as_bool)
}

fn option_duration_secs(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
    default_secs: u64,
) -> Duration {
    let secs = option_u64(options, keys).unwrap_or(default_secs);
    Duration::from_secs(secs)
}

fn option_duration_secs_opt(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
) -> Option<Duration> {
    option_u64(options, keys)
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
}

fn option_u64(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
) -> Option<u64> {
    option_value(options, keys).and_then(|value| match value {
        toml::Value::Integer(n) if *n >= 0 => Some(*n as u64),
        toml::Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    })
}

fn option_u32(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
) -> Option<u32> {
    option_u64(options, keys).and_then(|n| u32::try_from(n).ok())
}

fn option_usize(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
) -> Option<usize> {
    option_u64(options, keys).and_then(|n| usize::try_from(n).ok())
}

fn option_size(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
) -> Option<usize> {
    option_value(options, keys).and_then(|value| match value {
        toml::Value::Integer(n) if *n >= 0 => usize::try_from(*n as u64).ok(),
        toml::Value::String(s) => parse_size(s).ok().and_then(|n| usize::try_from(n).ok()),
        _ => None,
    })
}

fn option_size_u32(
    options: &Option<std::collections::HashMap<String, toml::Value>>,
    keys: &[&str],
) -> Option<u32> {
    option_size(options, keys).and_then(|n| u32::try_from(n).ok())
}
