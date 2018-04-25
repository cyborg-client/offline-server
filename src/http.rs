//! This module sets up a HTTP server accepting incoming requests. A client may send a POST request to start and stop the server using specified parameters.
//!
//! The HTTP module is responsible for serving all HTTP requests. For each incoming request, the call function for the Service trait is called. The input parameter for this function is a request object containing all the information about the request. The function matches the path and HTTP method to branch out and process each request. The returned value is a boxed Future containing a response. This means that the response value can be resolved at once using the ok-future, or at a later time using a future. For the /start URI the server checks that the server is not already started. If it is, it returns an error immediately. In order to return both ok(error) and a future success, the function returns Either::A or Either::B.
//!
//! When a request to start or stop the server is received, and the server is in the correct state, the command is forwarded to the controller using the CommandTx channel along with a oneshot channel for receiving an ACK from the controller. When the controller is has ACKed, the HTTP server will send a successful response to the HTTP client.

use controller::{Command, CommandTx, Config, Running};
use futures::{Future, Stream};
use futures::future::{Either, ok};
use futures::sync::oneshot;
use hyper::{Method, Request, Response, StatusCode};
use hyper::server::{Http, Service};
use std::cell::Cell;
use std::net::SocketAddr;
use std::ops::Deref;
use std::rc::Rc;

/// Create an object that can be called from main.
pub struct Server {}

impl Server
{
    /// Create a new server.
    pub fn new() -> Server {
        Server {}
    }

    /// Run the server.
    pub fn run(self, addr: &SocketAddr, command_tx: CommandTx) {
        // The running variable states whether or not the server as a whole is in the running state or not.
        let running = Rc::new(Cell::new(false));
        let command_tx = Rc::new(command_tx);
        let listener = Http::new().bind(addr, move || Ok(HttpService { running: running.clone(), command_tx: command_tx.clone() })).unwrap();
        listener.run().unwrap();
    }
}

/// Create the HttpService that will be created for each HTTP request:
struct HttpService {
    running: Rc<Cell<Running>>,
    command_tx: Rc<CommandTx>,
}

impl Service for HttpService {
    type Request = Request;
    type Response = Response;
    type Error = ::hyper::Error;
    /// Let the service return a future of a response since we might not have a reply ready right away:
    type Future = Box<Future<Item=Self::Response, Error=Self::Error>>;

    /// Parse the request and generate a response.
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
                    // Try to read the POST data as a Config struct:
                    let config: Config = if let Ok(n) = ::serde_json::from_slice(b.as_ref()) {
                        n
                    } else {
                        println!("Error: {}", (String::from_utf8_lossy(b.as_ref())));

                        return Either::A(ok(Response::new().with_status(StatusCode::BadRequest)));
                    };

                    if config.sample_rate != 10000 {
                        return Either::A(ok(Response::new().with_status(StatusCode::NotImplemented)));
                    }

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
                let command_tx = self.command_tx.clone();
                Box::new(req.body().skip_while(|_| ok(true)).concat2().and_then(move |_| {
                    if running.get() {
                        running.set(false);
                        println!("Stopped server.");
                        let (reply_tx, reply_rx) = oneshot::channel();
                        command_tx.deref().send((Command::Stop, reply_tx)).unwrap();

                        Either::A(
                            reply_rx
                                .and_then(|_| {
                                    ok(Response::new())
                                })
                                .or_else(|_| {
                                    ok(Response::new().with_status(StatusCode::InternalServerError))
                                })
                        )
                    } else {
                        println!("Error: Can't stop stopped server.");
                        Either::B(ok(
                            Response::new()
                                .with_status(StatusCode::Locked)
                                .with_body("Server already stopped.")
                        ))
                    }
                }))
            },
            ("/stimulate", &Method::Post) => {
                Box::new(req.body().skip_while(|_| ok(true)).concat2().and_then(move |_| {
                    ok(Response::new())
                }))
            },
            _ => {
                Box::new(ok(Response::new().with_status(StatusCode::NotFound)))
            }
        }
    }
}