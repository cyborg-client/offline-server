extern crate hyper;
extern crate futures;
extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate tokio;
extern crate tokio_io;
extern crate bytes;

use futures::future::{Future, ok};
use hyper::{Method, StatusCode, Body};
use hyper::server::{Http, Request, Response, Service};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use futures::Stream;
use hyper::Chunk;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::net::TcpListener;
use tokio::net::Incoming;
use std::io::Result;
use std::string::ToString;
use tokio::executor::current_thread;
use std::io::Cursor;
use futures::sync::oneshot;
use tokio_io::AsyncWrite;
use std::collections::HashMap;
use std::rc::Rc;
use std::cell::Cell;
use tokio::executor::current_thread::{task_executor};
use std::ops::Deref;
use futures::future::Either;
use bytes::Bytes;

#[derive(Deserialize, Debug)]
struct Config {
    sample_rate: u32,
    segment_length: u32
}

impl Config {
    fn new() -> Config {
        Config {
            sample_rate: 0,
            segment_length: 0,
        }
    }
}

type Running = bool;

enum Command {
    Start(Config),
    Stop
}

type CommandTx = std::sync::mpsc::Sender<(Command, oneshot::Sender<()>)>;
type CommandRx = std::sync::mpsc::Receiver<(Command, oneshot::Sender<()>)>;

struct HttpService {
    running: Rc<Cell<Running>>,
    command_tx: Rc<CommandTx>,
}

pub type ResponseStream = Box<Stream<Item=Chunk, Error=hyper::Error>>;

impl Service for HttpService {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = Box<Future<Item=Self::Response, Error=Self::Error>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        match (req.path(), req.method()) {
            ("/", &Method::Get) => {
                Box::new(ok(
                    Response::new().with_body("Offline MEA server")
                ))
            },
            ("/start", &Method::Post) => {
                let running = self.running.clone();
                let command_tx = self.command_tx.clone();
                Box::new(req.body().concat2().and_then(move |b| {
                    let config: Config = if let Ok(n) = serde_json::from_slice(b.as_ref()) {
                        n
                    } else {
                        println!("Error: {}", (String::from_utf8_lossy(b.as_ref())));

                        return Either::A(ok(Response::new().with_status(StatusCode::BadRequest)));
                    };

                    if running.get() {
                        println!("Error: Already running!");

                        return Either::A(ok(
                            Response::new()
                            .with_status(StatusCode::Locked)
                            .with_body("Server already started.")
                        ));
                    }

                    running.set(true);
                    println!("Start: {:?}", config);

                    let (reply_tx, reply_rx) = oneshot::channel();
                    command_tx.deref().send((Command::Start(config), reply_tx)).unwrap();

                    Either::B(
                        reply_rx
                            .and_then(|_| {
                                ok(Response::new())
                            })
                            .or_else(|_| {
                                ok(Response::new().with_status(StatusCode::InternalServerError))
                            })
                    )
                }))
            },
            ("/stop", &Method::Post) => {
                let mut running = self.running.clone();
                Box::new(req.body().skip_while(|_| ok(true)).concat2().map(move |_| {
                    if running.get() {
                        running.set(false);
                        println!("Stopped server.");
                        Response::new()
                    } else {
                        println!("Error: Can't stop stopped server.");
                        Response::new()
                            .with_status(StatusCode::Locked)
                            .with_body("Server already stopped.")
                    }
                }))
            },
            _ => {
                Box::new(ok(Response::new().with_status(StatusCode::NotFound)))
            }
        }
    }
}

type ClientTx = futures::sync::mpsc::Sender<Bytes>;
type ClientRx = futures::sync::mpsc::Sender<Bytes>;

struct Controller {
    command_rx: CommandRx,
    clients: Arc<Mutex<HashMap<SocketAddr, ClientTx>>>,
}

impl Controller {
    fn run(self) {
        let mut config = None;

        loop {
            if let Ok((command, reply_tx)) = self.command_rx.try_recv() {
                match command {
                    Command::Start(c) => {
                        config = Some(c);
                    },
                    Command::Stop => {
                        config = None;
                        self.clients.lock().unwrap().clear();
                    }
                }

                reply_tx.send(()).unwrap();
            }
        }
    }
}


fn main() {
    // Create a thread handle vector on which to let main join:
    let mut threads = Vec::new();

    // Create channels for communication between HTTP server and MEA read thread:
    let (command_tx, command_rx) = std::sync::mpsc::channel();

    // Create an HTTP-server listening for start and stop requests.
    // If already running, return error.
    // Else return OK immediately.
    let http_addr = "0.0.0.0:1234".parse().unwrap();
    threads.push(thread::spawn(move || {
        let running = Rc::new(Cell::new(false));
        let command_tx = Rc::new(command_tx);
        let http_server = Http::new().bind(&http_addr, move || Ok(HttpService { running: running.clone(), command_tx: command_tx.clone() })).unwrap();
        http_server.run().unwrap();
    }));


    let clients = Arc::new(Mutex::new(HashMap::new()));
    let clients_clone = clients.clone();
    threads.push(thread::spawn(move || {
        let controller = Controller {
            command_rx,
            clients: clients_clone,
        };

        controller.run();
    }));


    // Create two client lists, waiting and receiving.


    // Create a TCP-server where all data will be sent.
    // When clients are connected, add them to a list of waiting clients.
    // For each new segment, move clients to the list of receiving clients.


    // Start thread reading MEA data and sending it on all receiving clients.

    // Finally join all server threads:
    for i in threads {
        i.join().unwrap();
    }

    println!("Hmm");
}