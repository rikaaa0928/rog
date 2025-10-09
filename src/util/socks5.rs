pub(crate) mod client_hello;
pub(crate) mod confirm;
pub(crate) mod parse_util;
pub(crate) mod request;
pub(crate) mod server_hello;

pub(crate) static NO_AUTH: u8 = 0;

pub(crate) static CMD_CONNECT: u8 = 1;
pub(crate) static CMD_UDP: u8 = 3;
// pub(crate) static UDP_ERROR_STR: &str = "udp cmd";
// pub(crate) static UDP_ERROR: io::Error = io::Error::new(io::ErrorKind::Other, "udp cmd");
