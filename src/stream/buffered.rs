use crate::def::{RunStream, RunReadHalf, RunWriteHalf};
use crate::util::RunAddr;
use std::io;

/// A stream that wraps another `RunStream` but prepends a buffer to the read side.
/// This is useful when a protocol needs to be detected by reading some initial bytes,
/// and then those bytes need to be passed on to the actual protocol handler.
pub struct BufferedStream<'a> {
    buffer: Vec<u8>,
    stream: &'a mut dyn RunStream,
}

impl<'a> BufferedStream<'a> {
    pub fn new(buffer: Vec<u8>, stream: &'a mut dyn RunStream) -> Self {
        Self { buffer, stream }
    }
}

#[async_trait::async_trait]
impl<'a> RunStream for BufferedStream<'a> {
    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        // Splitting is not supported because we are borrowing the underlying stream.
        // The handshake logic for HTTP and SOCKS5 does not use split.
        unimplemented!("Cannot split a BufferedStream");
    }

    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.buffer.is_empty() {
            let n = std::cmp::min(self.buffer.len(), buf.len());
            buf[..n].copy_from_slice(&self.buffer[..n]);
            self.buffer.drain(..n);
            Ok(n)
        } else {
            self.stream.read(buf).await
        }
    }

    async fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n_read = 0;
        while n_read < buf.len() {
            match self.read(&mut buf[n_read..]).await {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "EOF during read_exact",
                    ));
                }
                Ok(n) => n_read += n,
                Err(e) => return Err(e),
            }
        }
        Ok(n_read)
    }

    async fn handshake(&mut self) -> io::Result<Option<(RunAddr, String)>> {
        // The handshake is handled by the acceptor using this buffered stream.
        // Calling it on the underlying stream again would be incorrect.
        Err(io::Error::new(
            io::ErrorKind::Other,
            "Handshake called on a BufferedStream",
        ))
    }

    async fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        self.stream.write(buf).await
    }
}