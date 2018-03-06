#![feature(duration_from_micros)]
extern crate byteorder;
extern crate bytes;
extern crate csv;
extern crate futures;
extern crate hyper;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate tokio;
extern crate tokio_io;

use byteorder::{BigEndian, ByteOrder};
use bytes::{BufMut, Bytes, BytesMut};
use csv::ReaderBuilder;
use futures::future::{Either, Future, ok};
use futures::Stream;
use futures::sync::oneshot;
use hyper::{Chunk, Method, StatusCode};
use hyper::server::{Http, Request, Response, Service};
use std::cell::Cell;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::io::prelude::*;
use std::io::{BufReader, SeekFrom};
use std::net::SocketAddr;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::executor::current_thread;
use tokio::net::TcpListener;
use tokio_io::io::write_all;

#[derive(Deserialize, Debug)]
struct Config {
    sample_rate: u32,
    segment_length: u32
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

type ClientTx = futures::sync::mpsc::Sender<Bytes>;

type Clients = Arc<Mutex<HashMap<SocketAddr, ClientTx>>>;

#[derive(Deserialize)]
struct Sample {
    #[allow(unused)]
    timestamp: u64,
    values: Vec<i32>,
}

struct Controller {
    command_rx: CommandRx,
    clients: Clients,
    config: Option<Config>,
    samples: Vec<BufReader<fs::File>>,
    last_segment_finished: Instant,
}

impl Controller {
    fn new(command_rx: CommandRx, clients: Clients, filename: Option<String>) -> Controller {
        let mut samples_reader = Vec::new();

        match filename {
            Some(filename) => {
                let mut samples = Vec::new();

                for i in 0..60 {
                    let file = fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .truncate(true)
                        .create(true)
                        .open(format!(".{}.dat", i)).unwrap();

                    samples.push(file);
                }

                let mut reader = ReaderBuilder::new().has_headers(false).from_path(filename).unwrap();

                reader.deserialize().for_each(|elem| {
                    let sample: Sample = elem.unwrap();
                    for (i, &value) in sample.values.iter().enumerate() {
                        let mut network_bytes = [0u8; 4];
                        BigEndian::write_i32(&mut network_bytes, value);
                        samples[i].write_all(&network_bytes).unwrap();
                    }
                });

                for mut file in &samples {
                    file.seek(SeekFrom::Start(0)).unwrap();
                }

                for mut file in samples {
                    samples_reader.push(BufReader::new(file));
                }
            },
            None => {
                for i in 0..60 {
                    let file = fs::OpenOptions::new()
                        .read(true)
                        .open(format!(".{}.dat", i)).unwrap();

                    samples_reader.push(BufReader::new(file));
                }
            },
        };

        Controller {
            command_rx,
            clients,
            config: None,
            samples: samples_reader,
            last_segment_finished: Instant::now(),
        }
    }

    fn update_config(&mut self) {
        loop {
            let (mut command, mut reply_tx) = (None, None);
            match self.config {
                None => {
                    if let Ok((a, b)) = self.command_rx.recv() {
                        command = Some(a);
                        reply_tx = Some(b);
                    }
                },
                Some(_) => {
                    if let Ok((a, b)) = self.command_rx.try_recv() {
                        command = Some(a);
                        reply_tx = Some(b);
                    } else {
                        return;
                    }
                }
            }

            match command.unwrap() {
                Command::Start(config) => {
                    self.config = Some(config);
                    self.last_segment_finished = Instant::now();
                    reply_tx.unwrap().send(()).unwrap();
                    return;
                },
                Command::Stop => {
                    self.config = None;
                    self.clients.lock().unwrap().clear();
                    reply_tx.unwrap().send(()).unwrap();
                }
            }
        }
    }

    fn sleep_until(&self, instant: Instant) {
        let now = Instant::now();
        if instant > now {
            thread::sleep(instant - now);
        }
    }

    fn collect_segment(&mut self) -> Bytes {
        let config = if let Some(ref config) = self.config {
            config
        } else {
            panic!("Config not set.");
        };

        let mut result = BytesMut::with_capacity(config.segment_length as usize * std::mem::size_of::<i32>() * 60).writer();

        let mut bytes_buf_vec = vec![0u8; config.segment_length as usize * std::mem::size_of::<i32>()];
        for file in &mut self.samples {
            let mut bytes_buf = bytes_buf_vec.as_mut_slice();
            file.read_exact(&mut bytes_buf);
            result.write_all(&mut bytes_buf);
        }

        let result = result.into_inner().freeze();


        self.last_segment_finished += Duration::from_micros((config.segment_length * 1000000 / config.sample_rate) as u64);
        self.sleep_until(self.last_segment_finished);
        result
    }

    fn run(mut self) {
        loop {
            self.update_config();

            let segment = self.collect_segment();

            let mut broken_clients = Vec::new();
            let mut clients = self.clients.lock().unwrap();
            for (address, tx) in clients.iter_mut() {
                if let Err(_) = tx.try_send(segment.clone()) {
                    broken_clients.push(address.clone());
                }
            }

            for i in broken_clients.iter().rev() {
                clients.remove(i);
            }
        }
    }
}

struct TcpServer {
    clients: Clients,
    listener: TcpListener,
}

impl TcpServer {
    fn bind(addr: &SocketAddr) -> TcpServer {
        TcpServer {
            clients: Arc::new(Mutex::new(HashMap::new())),
            listener: TcpListener::bind(addr).unwrap(),
        }
    }

    fn get_clients(&self) -> Clients {
        self.clients.clone()
    }

    fn run(self) {
        let clients = self.clients.clone();
        let server = self.listener.incoming().for_each(move |stream| {
            let (tx, rx) = futures::sync::mpsc::channel(100);

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


fn main() {
    let mut args = std::env::args();
    let mode = args.nth(1).unwrap();

    let controller_input = match mode.as_str() {
        "cached" => None,
        "build" => Some(args.next().expect("No filename given.")),
        "clear" => {
            for i in 0..60 {
                let _ = fs::remove_file(format!(".{}.dat", i));
            }

            std::process::exit(0);
        },
        "help" => {
            println!(r#"Available commands:
    build <filename>.csv - Builds the cache using the supplied CSV file and runs the server.
    cached - Runs the server using the files built using "build <filename>.csv"
    clear - Clears the cached files.
    help - Shows this help information."#);
            std::process::exit(0);
        }
        _ => {
            eprintln!("Invalid arguments. Try running with help.");
            std::process::exit(1);
        },
    };


    // Create a thread handle vector on which to let main join:
    let mut threads = Vec::new();

    // Create channels for communication between HTTP server and MEA read thread:
    let (command_tx, command_rx) = std::sync::mpsc::channel();

//    let (a, b) = futures::sync::oneshot::channel();
//    command_tx.send((Command::Start(Config::new()), a));

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


    let tcp_addr = "0.0.0.0:12345".parse().unwrap();
    let tcp_server = TcpServer::bind(&tcp_addr);
    let clients = tcp_server.get_clients();
    threads.push(thread::spawn(move || {
        let controller = Controller::new(command_rx, clients, controller_input);
        controller.run();
    }));

    threads.push(thread::spawn(move || {
        tcp_server.run();
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