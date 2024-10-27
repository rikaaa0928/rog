use std::io::{Error, Result};
use std::time::Duration;
use tokio::spawn;
use tokio::time::sleep;
use crate::connector::tcp::TcpRunConnector;
use crate::def::{RunAcceptor, RunConnector, RunListener, RunReadHalf, RunStream, RunWriteHalf};
use crate::host::tcp::TcpRunListener;

#[tokio::test]
async fn test_tcp() -> Result<()> {
    spawn(async {
        let listener = TcpRunListener::listen("127.0.0.1:12345".to_string()).await?;
        println!("server listen");
        let (stream, addr) = listener.accept().await?;
        println!("Accepted connection from {}", addr);
        let _ = spawn(async move {
            let (mut r, mut w) = stream.split();
            listener.handshake(&r, &w).await?;
            let a = spawn(async move {
                let mut buf = vec![0u8; 10];
                let n = r.read(&mut buf).await?;
                println!("Read {} bytes: {:?}", n, &&buf[..n]);
                Ok::<(), Error>(())
            });
            let b = spawn(async move {
                let _ = w.write("abcd".to_string().as_bytes()).await?;
                Ok::<(), Error>(())
            });
            a.await?;
            b.await?;
            Ok::<(), Error>(())
        });
        sleep(Duration::from_secs(1)).await;
        Ok::<(), Error>(())
    });
    sleep(Duration::from_secs(1)).await;
    let stream = TcpRunConnector::new().connect("127.0.0.1:12345".to_string()).await?;
    let (mut r, mut w) = stream.split();
    let a = spawn(async move {
        let mut buf = vec![0u8; 10];
        let n = r.read(&mut buf).await?;
        println!("Read {} bytes: {:?}", n, &&buf[..n]);
        Ok::<(), Error>(())
    });
    let b = spawn(async move {
        let _ = w.write("abcd".to_string().as_bytes()).await?;
        Ok::<(), Error>(())
    });
    a.await?;
    b.await?;
    Ok(())
}