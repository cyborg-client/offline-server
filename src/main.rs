extern crate hyper;
extern crate futures;
extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate serde_derive;

use futures::future::{Future, result, ok};
use hyper::{Method, StatusCode, Body};
use hyper::header::{ContentLength};
use hyper::server::{Http, Request, Response, Service};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use futures::Stream;
use hyper::Chunk;

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
    type Error = hyper::Error;
    type Response = Response<ResponseStream>;
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
                Box::new(req.body().concat2().map(|b| {
                    let config: Config = if let Ok(n) = serde_json::from_slice(b.as_ref()) {
                        n
                    } else {
                        println!("Error: {}", (String::from_utf8_lossy(b.as_ref())));

                        return Response::new().with_status(StatusCode::BadRequest);
                    };

                    println!("Start: {:?}", config);

                    Response::new().with_body(Box::new(Body::from("Valid")) as ResponseStream)
                }))
            },
            ("/stop", &Method::Post) => {
                Box::new(ok(Response::new().with_body(Box::new(
                    Body::from("Stopped server.")
                ) as ResponseStream)))
            },
            _ => {
                Box::new(ok(Response::new().with_status(StatusCode::NotFound)))
            }
        }
    }
}

fn main() {
    // Create shared Settings state.
    let mut server_state = Arc::new(Mutex::new(ServerStateData::new()));

    // Create a thread handle vector on which to let main join:
    let mut threads = Vec::new();

    // Create an HTTP-server listening for start and stop requests.
    // If already running, return error.
    // Else return OK immediately.
    let addr = "0.0.0.0:1234".parse().unwrap();
    let server_state_clone = server_state.clone();
    threads.push(thread::spawn(move || {
        let http_server = Http::new().bind(&addr, move || Ok(HttpService { server_state: server_state_clone.clone() })).unwrap();
        http_server.run().unwrap();
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
}