extern crate hyper;
extern crate futures;

use futures::future::Future;
use hyper::header::ContentLength;
use hyper::server::{Http, Request, Response, Service};
use std::sync::Arc;

struct Settings {
    running: bool,
    sample_rate: u16,
    segment_length: u32
}

struct HttpService {
    settings: Arc<Settings>
}

impl Service for HttpService {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = Box<Future<Item=Self::Response, Error=Self::Error>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        Box::new(futures::future::ok(
            Response::new()
                .with_header(ContentLength(10))
                .with_body("Heisann :)")
        ))
    }
}

fn main() {
    // Create shared Settings state.
    let mut settings = Arc::new(Settings {
        running: false,
        sample_rate: 0,
        segment_length: 0
    });

    // Create an HTTP-server listening for start and stop requests.
    // If already running, return error.
    // Else return OK immediately.
    let addr = "0.0.0.0:1234".parse().unwrap();

    let http_server = Http::new().bind(&addr, move || Ok(HttpService { settings: settings.clone() })).unwrap();
    http_server.run().unwrap();

    // Create two client lists, waiting and receiving.

    // Create a TCP-server where all data will be sent.
    // When clients are connected, add them to a list of waiting clients.
    // For each new segment, move clients to the list of receiving clients.

    // Start thread reading MEA data and sending it on all receiving clients.
}