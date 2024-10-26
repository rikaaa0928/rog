use std::io::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use std::pin::Pin;
use std::future::Future;
use crate::def::{RunConnector, RunReadHalf, RunStream, RunWriteHalf};

pub struct TcpReadHalf {
    reader: tokio::net::tcp::OwnedReadHalf,
}

// TcpStream 实现的写半边
pub struct TcpWriteHalf {
    writer: tokio::net::tcp::OwnedWriteHalf,
}

// TcpStream 的包装
pub struct TcpRunStream {
    inner: TcpStream,
}

pub struct TcpConnector {}

// 为 TcpReadHalf 实现 MyReadHalf trait
impl RunReadHalf for TcpReadHalf {
    type ReadFuture<'a> = Pin<Box<dyn Future<Output=Result<usize>> + Send + 'a>>;

    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::ReadFuture<'a> {
        Box::pin(async move {
            self.reader.read(buf).await
        })
    }
}

// 为 TcpWriteHalf 实现 MyWriteHalf trait
impl RunWriteHalf for TcpWriteHalf {
    type WriteFuture<'a> = Pin<Box<dyn Future<Output=Result<()>> + Send + 'a>>;

    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Self::WriteFuture<'a> {
        Box::pin(async move {
            self.writer.write_all(buf).await
        })
    }
}

// 为 MyTcpStream 实现构造方法
impl TcpRunStream {
    pub fn new(stream: TcpStream) -> Self {
        Self { inner: stream }
    }
}

// 为 MyTcpStream 实现 MyStream trait
impl RunStream for TcpRunStream {
    type ReadHalf = TcpReadHalf;
    type WriteHalf = TcpWriteHalf;

    fn split(self) -> (Self::ReadHalf, Self::WriteHalf) {
        let (reader, writer) = self.inner.into_split();
        (
            TcpReadHalf { reader },
            TcpWriteHalf { writer },
        )
    }
}
impl TcpConnector {
    pub fn new() -> Self {
        TcpConnector {}
    }
}
impl RunConnector for TcpConnector {
    type Stream = TcpRunStream;
    type StreamFuture = Pin<Box<dyn Future<Output=Result<Self::Stream>>>>;

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
    use std::thread::sleep;
    use std::time::Duration;
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
        sleep(Duration::from_millis(100));
        // Use TcpConnector to connect to the server
        let connector = TcpConnector::new();
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