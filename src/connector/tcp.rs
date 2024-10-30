use std::io::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use std::pin::Pin;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::def::{RunConnector, RunReadHalf, RunStream, RunUdpConnector, RunUdpStream, RunWriteHalf};
use crate::stream::tcp::TcpRunStream;
use crate::stream::udp::UdpRunStream;

pub struct TcpRunConnector {}

impl TcpRunConnector {
    pub fn new() -> Self {
        TcpRunConnector {}
    }
}

impl RunUdpConnector for TcpRunConnector {
    type UdpStream = UdpRunStream;
    type UdpFuture = Pin<Box<dyn Future<Output=Result<Option<Self::UdpStream>>> + Send>>;
    fn udp_tunnel(&self, addr: SocketAddr) -> Self::UdpFuture {
        Box::pin(async move {
            Ok(None)
        })
    }
}

impl RunUdpConnector for Arc<Mutex<TcpRunConnector>> {
    type UdpStream = UdpRunStream;
    type UdpFuture = Pin<Box<dyn Future<Output=Result<Option<Self::UdpStream>>> + Send>>;

    fn udp_tunnel(&self, listen_addr: SocketAddr) -> Self::UdpFuture {
        let connector = self.clone();
        Box::pin(async move {
            let connector = connector.lock().await;
            connector.udp_tunnel(listen_addr).await
        })
    }
}

impl RunConnector for TcpRunConnector {
    type Stream = TcpRunStream;
    type StreamFuture = Pin<Box<dyn Future<Output=Result<Self::Stream>> + Send>>;
    fn connect(&self, addr: String) -> Self::StreamFuture {
        Box::pin(async move {
            let tcp_stream = TcpStream::connect(addr).await?;
            Ok(TcpRunStream::new(tcp_stream))
        })
    }
}

mod test {
    use std::result;
    use tokio::net::TcpListener;
    use tokio::spawn;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::task::JoinHandle;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use std::time::Duration;
    use tokio::time::sleep;
    use super::*;
    #[tokio::test]
    async fn test_tcp_connector() -> Result<()> {
        // Start a TCP server
        let server_addr = "127.0.0.1:0".to_string(); // Use port 0 to let the OS assign a port
        let listener = TcpListener::bind(&server_addr).await?;
        let local_addr = listener.local_addr()?;
        let server_running = Arc::new(AtomicBool::new(true));

        let _: JoinHandle<Result<()>> = {
            let server_running = server_running.clone();
            spawn(async move {
                while server_running.load(Ordering::SeqCst) {
                    let x = listener.accept().await;
                    if x.is_err() {
                        return Ok(());
                    }
                    let (mut socket, _) = x.unwrap();
                    spawn(async move {
                        let mut buf = vec![0; 1024];
                        let x = socket.read(&mut buf).await;
                        if x.is_err() {
                            return Ok(());
                        }
                        let n = x.unwrap();
                        socket.write_all(&buf[..n]).await;
                        return Ok::<(), Error>(());
                    });
                }
                Ok(())
            })
        };
        sleep(Duration::from_millis(100)).await;
        // Use TcpConnector to connect to the server
        let connector = TcpRunConnector::new();
        let addr = format!("{}", local_addr);
        let stream = connector.connect(addr).await?;

        let (mut read_half, mut write_half) = stream.split();

        // Spawn a new task to read the response
        let read_handle: JoinHandle<result::Result<Vec<u8>, Box<dyn std::error::Error + Send>>> = spawn(async move {
            let mut buf = vec![0; 1024];
            let x = read_half.read(&mut buf).await;
            let n = x.unwrap();
            Ok(buf[..n].to_vec())
        });

        // Send a message
        let message = b"Hello, world!";
        write_half.write(message).await?;
        let x = read_handle.await?;
        if x.is_err() {
            return Ok(());
        }
        // Wait for the response
        let response = x.unwrap();
        println!("Got response: {:?}", &response);
        // Verify the response
        assert_eq!(response, message);

        // Stop the server
        server_running.store(false, Ordering::SeqCst);
        // drop(listener); // Close the listener to stop accepting new connections
        // server_handle.await??;

        Ok(())
    }
}