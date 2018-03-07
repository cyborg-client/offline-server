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

pub struct Server {}

impl Server
{
    pub fn new() -> Server {
        Server {}
    }

    pub fn run(self, addr: &SocketAddr, command_tx: CommandTx) {
        let running = Rc::new(Cell::new(false));
        let command_tx = Rc::new(command_tx);
        let listener = Http::new().bind(addr, move || Ok(HttpService { running: running.clone(), command_tx: command_tx.clone() })).unwrap();
        listener.run().unwrap();
    }
}

struct HttpService {
    running: Rc<Cell<Running>>,
    command_tx: Rc<CommandTx>,
}

impl Service for HttpService {
    type Request = Request;
    type Response = Response;
    type Error = ::hyper::Error;
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
            _ => {
                Box::new(ok(Response::new().with_status(StatusCode::NotFound)))
            }
        }
    }
}