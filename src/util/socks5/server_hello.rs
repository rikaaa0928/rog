use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use crate::def::{RunReadHalf, RunStream};
use crate::stream::tcp::{TcpReadHalf};

struct Socks5ServerHello {
    version: u8,
    method: u8,
}

impl Socks5ServerHello {
    fn new(version: u8, method: u8) -> Self {
        Socks5ServerHello {
            version,
            method,
        }
    }
    fn to_bytes(&self) -> Vec<u8> {
        let mut res = Vec::new();
        res.push(self.version);
        res.push(self.method);
        res
    }
}