extern crate hyper;
extern crate futures;
extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
extern crate tokio;
extern crate tokio_io;

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
use tokio_io::AsyncRead;
use std::ops::{DerefMut, Deref};
use tokio::executor::current_thread;
use tokio_io::AsyncWrite;
use std::io::Write;

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

struct ServerStateData {
    running: bool,
    config: Config
}

impl ServerStateData {
    fn new() -> ServerStateData {
        ServerStateData {
            running: false,
            config: Config::new()
        }
    }
}

type ServerState = Arc<Mutex<ServerStateData>>;

struct HttpService {
    server_state: ServerState
}

pub type ResponseStream = Box<Stream<Item=Chunk, Error=hyper::Error>>;

impl Service for HttpService {
    type Request = Request;
    type Response = Response<ResponseStream>;
    type Error = hyper::Error;
    type Future = Box<Future<Item=Self::Response, Error=Self::Error>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        match (req.path(), req.method()) {
            ("/", &Method::Get) => {
                Box::new(ok(
                    Response::new().with_body(Box::new(
                        Body::from("Offline MEA server")
                    ) as ResponseStream)
                ))
            },
            ("/start", &Method::Post) => {
                let server_state = self.server_state.clone();
                Box::new(req.body().concat2().map(move |b| {
                    let config: Config = if let Ok(n) = serde_json::from_slice(b.as_ref()) {
                        n
                    } else {
                        println!("Error: {}", (String::from_utf8_lossy(b.as_ref())));

                        return Response::new().with_status(StatusCode::BadRequest);
                    };

                    {
                        let mut server_state = server_state.lock().expect("ERROR");

                        if server_state.running {
                            println!("Error: Already running!");

                            return Response::new()
                                .with_status(StatusCode::Locked)
                                .with_body(Box::new(Body::from("Server already started.")) as ResponseStream)
                        }
                        server_state.running = true;
                        server_state.config = config;

                        println!("Start: {:?}", server_state.config);
                    }

                    Response::new()
                }))
            },
            ("/stop", &Method::Post) => {
                {
                    let mut server_state = self.server_state.lock().expect("ERROR");

                    if server_state.running {
                        println!("Stopped server.");
                        server_state.running = false;
                    } else {
                        println!("Error: Can't stop stopped server.");
                        return Box::new(ok(Response::new().with_status(StatusCode::BadRequest)));
                    }
                }

                Box::new(req.body().skip_while(|_| ok(true)).concat2().map(|_| Response::new()))
            },
            _ => {
                Box::new(ok(Response::new().with_status(StatusCode::NotFound)))
            }
        }
    }
}


struct TcpServer {
    waiting: Arc<Mutex<Vec<TcpStream>>>,
    receiving: Arc<Mutex<Vec<TcpStream>>>,
    incoming: Incoming,
    server_state: ServerState,
}

impl TcpServer {
    fn bind(addr: &SocketAddr, server_state: ServerState) -> Result<TcpServer> {
        let incoming = TcpListener::bind(addr)?.incoming();

        Ok(TcpServer {
            waiting: Arc::new(Mutex::new(Vec::new())),
            receiving: Arc::new(Mutex::new(Vec::new())),
            incoming,
            server_state,
        })
    }

    fn run(self) -> Result<()> {
        let waiting_clone = self.waiting.clone();
        let server = self.incoming.for_each(move |socket| {
            waiting_clone.lock().expect("ERROR").push(socket);
            Ok(())
        }).map_err(|err| {
            println!("accept error = {:?}", err);
        });

        let mut threads = Vec::new();
        threads.push(thread::Builder::new().name("TCP socket acceptor".to_string()).spawn(move || {
            current_thread::run(|_| {
                current_thread::spawn(server);
            });
        }).expect("ERROR"));

        let server_state = self.server_state.clone();
        let receiving = self.receiving.clone();
        let waiting_clone = self.waiting.clone();
        threads.push(thread::Builder::new().name("TCP data sender".to_string()).spawn(move || {
            loop {
                let current_state;
                {
                    current_state = server_state.lock().expect("ERROR");
                }

                if !current_state.running {
                    let mut receiving = receiving.lock().expect("ERROR");
//                    if receiving.deref().len() != 0 {
//                        for mut stream in receiving.deref_mut() {
//                            stream.shutdown().expect("ERROR");
//                        }
//                    }
                    receiving.clear();
                    // TODO: Use Condvar instead.
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }

                {
                    let mut waiting = waiting_clone.lock().expect("ERROR");
                    let mut receiving = receiving.lock().expect("ERROR");
                    while waiting.len() != 0 {
                        receiving.push(waiting.pop().expect("Couldn't pop from waiting list."));
                    }
                }

                let mut receiving = receiving.lock().expect("ERROR");
                let mut close = Vec::new();
                for (i, stream) in receiving.iter_mut().enumerate() {
                    match stream.write(b"HeiHai") {
                        Err(e) => {
                            println!("Couldn't write: {}", e);
                            close.push(i);
                            continue;
                        },
                        _ => {}
                    }
                    //thread::sleep(Duration::from_millis(500));
                }

                for i in close.into_iter().rev() {
                    receiving.remove(i);
                }


            }
        }).expect("ERROR"));

        for thread in threads {
            thread.join().expect("ERROR");
        }

        println!("Hmm");

        Ok(())
    }
}

fn main() {
    // Create shared Settings state.
    let server_state = Arc::new(Mutex::new(ServerStateData::new()));

    // Create a thread handle vector on which to let main join:
    let mut threads = Vec::new();

    // Create an HTTP-server listening for start and stop requests.
    // If already running, return error.
    // Else return OK immediately.
    let http_addr = "0.0.0.0:1234".parse().expect("ERROR");
    let server_state_clone = server_state.clone();
    threads.push(thread::spawn(move || {
        let http_server = Http::new().bind(&http_addr, move || Ok(HttpService { server_state: server_state_clone.clone() })).expect("ERROR");
        http_server.run().expect("ERROR");
    }));

    // Create two client lists, waiting and receiving.


    // Create a TCP-server where all data will be sent.
    // When clients are connected, add them to a list of waiting clients.
    // For each new segment, move clients to the list of receiving clients.
    let tcp_addr = "0.0.0.0:12345".parse().expect("ERROR");
    let server_state_clone = server_state.clone();
    let listener = TcpServer::bind(&tcp_addr, server_state_clone).expect("ERROR");
    threads.push(thread::spawn(move || {
        listener.run().expect("ERROR")
    }));

    // Start thread reading MEA data and sending it on all receiving clients.

    // Finally join all server threads:
    for i in threads {
        i.join().expect("ERROR");
    }

    println!("Hmm");
}