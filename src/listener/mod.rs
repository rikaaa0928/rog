use crate::def::{config, RunAcceptor, RunListener};
use crate::listener::grpc::GrpcListener;
use crate::listener::socks5::SocksRunAcceptor;
use crate::listener::tcp::TcpRunListener;
use crate::object::config::ObjectConfig;

pub(crate) mod tcp;
pub(crate) mod socks5;
mod grpc;

pub async fn create(cfg: &ObjectConfig) -> std::io::Result<Box<dyn RunAcceptor>> {
    match cfg.listener.proto.as_str() {
        "socks5" => {
            let listener = TcpRunListener {}.listen(cfg.listener.endpoint.as_str()).await?;
            let socks5 = Box::new(SocksRunAcceptor::new(listener, None, None));
            Ok(socks5)
        }
        "grpc" => {
            let grpc = GrpcListener::new(cfg.clone());
            let acc = grpc.listen(cfg.listener.endpoint.as_str()).await?;
            Ok(acc)
        }
        _ => {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("listener proto {} not found", cfg.listener.proto.as_str())))
        }
    }
}