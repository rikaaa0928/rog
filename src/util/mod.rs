use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use crate::util::socks5::request::Request;
use std::str;
use crate::util::socks5::CMD_UDP;

pub(crate) mod socks5;

#[derive(Debug,Clone)]
pub struct RunAddr {
    pub addr: String,
    pub port: u16,
    // pub a_type: u8,
    pub udp: bool,
    pub cache: Option<Vec<u8>>,
}

impl RunAddr {
    pub(crate) fn endpoint(&self) -> String {
        format!("{}:{}", self.addr, self.port)
    }
}

impl TryFrom<&Request> for RunAddr {
    type Error = std::io::Error;

    fn try_from(value: &Request) -> std::io::Result<Self> {
        let port = u16::from_be_bytes(value.dst_port);
        let addr = match value.a_typ {
            // IPv4
            1 => {
                if value.dst_addr.len() != 4 {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, "Not a ipv4"));
                }
                let ip = Ipv4Addr::new(
                    value.dst_addr[0],
                    value.dst_addr[1],
                    value.dst_addr[2],
                    value.dst_addr[3],
                );
                Ok(ip.to_string())
            }
            // Domain name
            3 => {
                match str::from_utf8(&value.dst_addr) {
                    Ok(domain) => Ok(domain.to_string()),
                    Err(_) => Err(std::io::Error::new(std::io::ErrorKind::Other, "Not a domain"))
                }
            }
            // IPv6
            4 => {
                if value.dst_addr.len() != 16 {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, "Not a ipv6"));
                }
                let ip = Ipv6Addr::new(
                    u16::from_be_bytes([value.dst_addr[0], value.dst_addr[1]]),
                    u16::from_be_bytes([value.dst_addr[2], value.dst_addr[3]]),
                    u16::from_be_bytes([value.dst_addr[4], value.dst_addr[5]]),
                    u16::from_be_bytes([value.dst_addr[6], value.dst_addr[7]]),
                    u16::from_be_bytes([value.dst_addr[8], value.dst_addr[9]]),
                    u16::from_be_bytes([value.dst_addr[10], value.dst_addr[11]]),
                    u16::from_be_bytes([value.dst_addr[12], value.dst_addr[13]]),
                    u16::from_be_bytes([value.dst_addr[14], value.dst_addr[15]]),
                );
                Ok(ip.to_string())
            }
            _ => Err(std::io::Error::new(std::io::ErrorKind::Other, "a_type not found"))
        }?;

        Ok(Self {
            addr,
            port,
            // a_type: value.a_typ,
            udp: value.cmd == CMD_UDP,
            cache: None,
        })
    }
}

// pub fn split_vec_into_chunks(data: Vec<u8>, chunk_size: usize) -> Vec<Vec<u8>> {
//     data.chunks(chunk_size)
//         .map(|chunk| chunk.to_vec())
//         .collect()
// }

// impl TryInto<SocketAddr> for RunAddr {
//     type Error = std::io::Error;
// 
//     fn try_into(self) -> Result<SocketAddr, Self::Error> {
//         match self.a_type {
//             // IPv4
//             1 => {
//                 // 尝试将字符串解析为 IPv4 地址
//                 match self.addr.parse() {
//                     Ok(ip) => Ok(SocketAddr::new(ip, self.port)),
//                     Err(_) => Err(std::io::Error::new(
//                         std::io::ErrorKind::InvalidInput,
//                         "Invalid IPv4 address",
//                     ))
//                 }
//             }
//             // Domain
//             3 => {
//                 // 对于域名，我们需要进行DNS解析
//                 let addr_port = format!("{}:{}", self.addr, self.port);
//                 match addr_port.to_socket_addrs() {
//                     Ok(mut addrs) => addrs.next().ok_or_else(|| {
//                         std::io::Error::new(
//                             std::io::ErrorKind::NotFound,
//                             "Could not resolve domain name",
//                         )
//                     }),
//                     Err(e) => Err(e)
//                 }
//             }
//             // IPv6
//             4 => {
//                 // 尝试将字符串解析为 IPv6 地址
//                 match self.addr.parse() {
//                     Ok(ip) => Ok(SocketAddr::new(ip, self.port)),
//                     Err(_) => Err(std::io::Error::new(
//                         std::io::ErrorKind::InvalidInput,
//                         "Invalid IPv6 address",
//                     ))
//                 }
//             }
//             // 未知类型
//             _ => Err(std::io::Error::new(
//                 std::io::ErrorKind::InvalidInput,
//                 "Unknown address type",
//             ))
//         }
//     }
// }