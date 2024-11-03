use crate::def::{config, RunAcceptor, RunListener};
use crate::listener::socks5::SocksRunAcceptor;
use crate::listener::tcp::TcpRunListener;

pub(crate) mod tcp;
pub(crate) mod socks5;

pub async fn create(cfg: &config::Listener) -> std::io::Result<Box<dyn RunAcceptor>> {
    match cfg.proto.as_str() {
        "socks5" => {
            let listener = TcpRunListener::listen(cfg.endpoint.as_str()).await?;
            let socks5 = Box::new(SocksRunAcceptor::new(listener, None, None));
            Ok(socks5)
        }
        _ => {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("listener proto {} not found", cfg.proto.as_str())))
        }
    }
}