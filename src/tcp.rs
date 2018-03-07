use bytes::Bytes;
use futures::{Future, Stream};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::executor::current_thread;
use tokio::net::TcpListener;
use tokio_io::io::write_all;

pub type ClientTx = ::futures::sync::mpsc::Sender<Bytes>;
pub type Clients = Arc<Mutex<HashMap<SocketAddr, ClientTx>>>;

pub struct Server {
    clients: Clients,
    listener: TcpListener,
}

impl Server {
    pub fn bind(addr: &SocketAddr) -> Server {
        Server {
            clients: Arc::new(Mutex::new(HashMap::new())),
            listener: TcpListener::bind(addr).unwrap(),
        }
    }

    pub fn get_clients(&self) -> Clients {
        self.clients.clone()
    }

    pub fn run(self) {
        let clients = self.clients.clone();
        let server = self.listener.incoming().for_each(move |stream| {
            let (tx, rx) = ::futures::sync::mpsc::channel(100000);

            {
                clients.lock().unwrap().insert(stream.peer_addr().unwrap(), tx);
            }

            let writer = rx.fold(stream, |stream, msg| {
                write_all(stream, msg)
                    .map(|(stream, _)| stream)
                    .map_err(|_| ())
            }).map(|_| ());

            current_thread::spawn(writer);

            Ok(())
        }).map_err(|_| ());

        current_thread::run(|_| {
            current_thread::spawn(server);
        });
    }
}