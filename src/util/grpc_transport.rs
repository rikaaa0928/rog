use hyper_util::rt::TokioIo;
use std::io;

use tokio::net::TcpStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

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

    endpoint
        .connect_with_connector(service_fn(move |uri: Uri| {
            let tcp_nodelay = tcp_nodelay;

            async move {
                let host = uri.host().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "grpc endpoint missing host")
                })?;
                let port = uri.port_u16().unwrap_or_else(|| default_port(&uri));
                let stream = TcpStream::connect((host, port)).await?;

                stream.set_nodelay(tcp_nodelay)?;

                Ok::<_, io::Error>(TokioIo::new(stream))
            }
        }))
        .await
}
