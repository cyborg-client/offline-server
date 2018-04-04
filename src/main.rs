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

mod tcp; // Manages the TCP server.
mod http; // Manages the HTTP server and handles HTTP connections.
mod controller; // Pipes data from the CSV file to the TCP clients.

use std::{fs, thread};
use controller::Controller;

// Struct to represent the config.json-file.
#[derive(Deserialize)]
struct Config {
    http_port: u16,
    tcp_port: u16,
}

fn main() {
    // Parse the input arguments:
    let mut args = std::env::args();
    let mode = args.nth(1).unwrap();

    let mut controller_input = None;
    match mode.as_str() {
        "run" => controller_input = None,
        "build" => controller_input = Some(args.next().expect("No filename given.")),
        "clear" => {
            for i in 0..60 {
                let _ = fs::remove_file(format!(".{}.dat", i));
            }
        },
        "help" => {
            println!(r#"Available commands:
    build <filename>.csv - Builds the cache using the supplied CSV file.
    run - Runs the server using the files built using "build <filename>.csv"
    clear - Clears the cached files.
    help - Shows this help information."#);
        }
        _ => {
            eprintln!("Invalid arguments. Try running with help.");
            std::process::exit(1);
        },
    };

    // If the controller hasn't gotten any input, quit:
    if controller_input == None {
        std::process::exit(0);
    }


    // Get the ports from the config file:
    let config_file = fs::File::open("config.json").expect("Could not open config.json");
    let config: Config = serde_json::from_reader(config_file).expect("The config is missing one or more element.");

    // Create a thread handle vector on which to let main join:
    let mut threads = Vec::new();

    // Create channels for communication between HTTP handler and Controller thread:
    let (command_tx, command_rx) = std::sync::mpsc::channel();

    // Start HTTP handler:
    let http_addr = (String::from("0.0.0.0:") + &config.http_port.to_string()).parse().unwrap();
    threads.push(thread::spawn(move || {
        http::Server::new().run(&http_addr, command_tx);
    }));

    // Start the TCP connection manager:
    let tcp_addr = (String::from("0.0.0.0:") + &config.tcp_port.to_string()).parse().unwrap();
    let tcp_server = tcp::Server::bind(&tcp_addr);

    // The TCP server keeps a list of connected clients. Get a reference to this list:
    let clients = tcp_server.get_clients();

    threads.push(thread::spawn(move || {
        tcp_server.run();
    }));

    // Create and start the Controller:
    threads.push(thread::spawn(move || {
        let controller = Controller::new(command_rx, clients, controller_input);
        controller.run();
    }));


    // Finally join on all threads:
    for i in threads {
        i.join().unwrap();
    }
}