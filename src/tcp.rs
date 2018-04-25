//! This module sets up a TCP server. When a client is connected it will receive segments when the server is running. When a HTTP stop request is received, all TCP clients are kicked. Each TCP client connection gets its own TCP client manager that sends segments to the clients.
//!
//! For each TCP client that connects to the TCP server the server creates a RX-TX pair for sending and receiving segments. The TX part is added to the clients list so the controller can send segments to it. Then we create a stream future reading from the RX and piping the segments onto the network. This stream future is then driven by the current_thread executor in the Tokio library.

use bytes::Bytes;
use futures::{Future, Stream};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::executor::current_thread;
use tokio::net::TcpListener;
use tokio_io::io::write_all;

/// The type for transmitting data to a TCP client.
pub type ClientTx = ::futures::sync::mpsc::Sender<Bytes>;

/// Represents a HashMap of Clients. It will have their addresses and their transmit side of the channel.
pub type Clients = Arc<Mutex<HashMap<SocketAddr, ClientTx>>>;

/// Represents the state of the server.
pub struct Server {
    clients: Clients,
    listener: TcpListener,
}

impl Server {

    /// Binds the TCP server to the specified address.
    pub fn bind(addr: &SocketAddr) -> Server {
        Server {
            clients: Arc::new(Mutex::new(HashMap::new())),
            listener: TcpListener::bind(addr).unwrap(),
        }
    }

    /// Gets a reference counted reference to the clients HashMap.
    pub fn get_clients(&self) -> Clients {
        self.clients.clone()
    }

    /// Runs the server, setting up the green threaded event loop.
    ///
    /// The TCP client manager (event loop) will read any data on the Client channel and output it onto the TCP connection.
    pub fn run(self) {
        let clients = self.clients.clone();
        let server = self.listener.incoming().for_each(move |stream| {
            let (tx, rx) = ::futures::sync::mpsc::channel(10000000);

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