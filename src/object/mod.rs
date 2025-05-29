use crate::def::{RouterSet, RunAccStream};
use crate::object::config::ObjectConfig;
use crate::{connector, listener};
use log::{debug, error};
use std::io;
use std::io::Error;
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::Mutex;

pub mod config;
pub mod tcp;
pub mod udp;
pub mod raw_udp;

pub struct Object {
    config: Arc<ObjectConfig>,
    router: Arc<dyn RouterSet>,
}

impl Object {
    pub fn new(config: Arc<ObjectConfig>, router: Arc<dyn RouterSet>) -> Self {
        Self { config, router }
    }

    pub async fn start(&self) -> io::Result<()> {
        let config = self.config.clone();
        let router = self.router.clone();
        let acc = listener::create(&config, router.clone()).await?;
        let main_acceptor = Arc::new(acc);

        loop {
            let (acc_stream, _) = main_acceptor.accept().await?;
            let main_acceptor = Arc::clone(&main_acceptor);
            let router = Arc::clone(&router);
            let config = config.clone();
            spawn(async move {
                match acc_stream {
                    RunAccStream::TCPStream(tcp_stream) => {
                        let (mut r, mut w) = tcp_stream.split();
                        // let udp_tunnel=Arc::new(&connector).lock().await.udp_tunnel();
                        let addr_res = main_acceptor.handshake(r.as_mut(), w.as_mut()).await;
                        match addr_res {
                            Err(e) => {
                                error!("Handshake error: {}", e);
                            }
                            Ok(addr) => {
                                let addr_ref = &addr;

                                if addr_ref.udp {
                                    udp::handle_udp_connection(
                                        r,
                                        w,
                                        Arc::clone(&main_acceptor),
                                        config.clone(),
                                        router.clone(),
                                        addr,
                                    )
                                        .await?;
                                } else {
                                    let client_name = router
                                        .route(
                                            config.listener.name.as_str(),
                                            config.listener.router.as_str(),
                                            addr_ref,
                                        )
                                        .await;
                                    let conn_conf = config.connector.get(client_name.as_str()).unwrap();
                                    let connector_obj =
                                        Arc::new(Mutex::new(connector::create(conn_conf).await?));
                                    debug!("Handshake successful {:?}", addr_ref);
                                    let client_stream_res = Arc::clone(&connector_obj)
                                        .lock()
                                        .await
                                        .connect(addr_ref.endpoint())
                                        .await;
                                    let mut error = false;
                                    if client_stream_res.is_err() {
                                        error = true;
                                    }
                                    let client_stream = client_stream_res?;
                                    main_acceptor
                                        .post_handshake(r.as_mut(), w.as_mut(), error, 0)
                                        .await?;
                                    tcp::handle_tcp_connection(r, w, addr, client_stream).await?;
                                }
                            }
                        }
                        Ok::<(), Error>(())
                    }
                    RunAccStream::UDPSocket((r,w)) => {
                        raw_udp::handle_rwa_udp(r, w, config, router.clone()).await
                    }
                }
            });
        }
        Ok(())
    }
}
