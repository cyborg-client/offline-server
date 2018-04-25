//! # Offline MEA Server
//! The offline server implements the same API as the MEA server, however instead of getting its data from the MEA it gets its data from an offline CSV file. This means it can be used instead of the MEA server when the MEA server is not available (e.g. poor network connection at conferences or that the MEA is not connected to the server).

//! The server is written in Rust. To follow some of the more technical aspects of the discussion below, it is recommended to have an idea of what the Rust ownership system is. The following discussion is a bit simplified to be easy to follow for people who have not written specifically in Rust.
//!
//! ## Overview over memory management
//!
//! We now turn to the memory management problem of the server. The server listens to HTTP connections for configuring the server state. The state is whether the server is running, the segment length and the sample rate of transmission. When the server is running, it is accepting TCP connections. When a client connects it will start receiving data. The first byte it receives will be from a segment boundary. This means that if the server is currently recording in the middle of a segment, the server will wait until a new segment boundary is started before sending data to this new client.
//!
//! Since the TCP data is streamed over the internet, there might be temporary disruptions in the connection and other issues that cause the OS's TCP sending buffer to fill completely. To stop this from disconnecting clients as soon as this happens, we want to buffer the MEA data for as long as needed (up to a certain threshold where the client is kicked for being too slow at receiving). When all the connected clients have received a given segment it should go out of scope to free the memory.
//!
//! To achieve this, we have created a queue of segments for each TCP client. Since we do not know at compile time which clients will connect, we cannot prove who will get a reader reference, so we will not know when there are zero references to the data, and the compiler cannot infer when to delete the data. To allow for having multiple readers created on runtime, we can use a reference counted wrapper for the segment data. This counts all the created references on runtime, and decreases the counter when a reference goes out of scope. This upholds the invariants demanded by the ownership system on runtime. This means there will be a small but necessary runtime penalty for doing this.
//!
//! At last we can describe how the memory of the segments are managed. In short a controller module generates segments which are wrapped in reference counted objects. Each of these segments are then sent on a bounded buffered channel to each TCP client manager. If the bounded buffer is full the TCP connection will be closed. In other words if the TCP backpressure gets too large, we terminate the connection. Each TCP client manager reads from the channel, sends the segments onto the network. Since each segment is sent as a whole to each TCP client manager we ensure that each TCP client receives data from a segment boundary. When the data is sent on the network the segment goes out of scope. When all TCP client managers have sent the data and let the segment reference go out of scope, there are zero references to the segment, and it is then deleted by the ownership system. If a channel gets full the TCP connection is closed, and the buffer is deleted. This means all the contained segment references go out of scope, and finally the segments are deleted. In other words all segments are stored for as long as needed, and deleted as soon as possible keeping the memory footprint as low as possible.
//!
//! ## Architecture and language for Offline MEA Server
//! ### Why Rust?
//! We decided to use Rust for our server because it runs as fast as highly optimized C++ code. In addition it only accepts safe code so that we were much less likely to encounter runtime bugs. This was an important trait for this project as we did not have much time to do runtime debugging. Instead we could rely on Rust's safe type system to eliminate most of the bugs. This means that when the program compiles and runs without a bug once, it will likely run without any bugs for a long time.
//!
//! Considering the above one might wonder why we used Rust only for the server, and not for the client program. One important reason is that in order for Rust to accomplish its speed and safety it enforces a strict type system that takes some time to learn. If the code cannot be proved by the compiler to be safe, it will not compile. This makes it unsuitable for use by people who do not already know Rust or are not experienced C++ programmers (or experienced in any other language that supports raw pointers). Since only one person was already fluent in Rust, he was assigned the responsibility of the offline MEA server.
//!
//! ### Architectural choices
//! #### The segment memory management structure
//!
//! We chose the memory management technique described because it is a very easy solution to the problem. However it also means that each segment gets its own object on the heap. This is fairly expensive because the segments will not be located in the CPU Lx-caches, resulting in an allocation and usage cost of about 200 times longer than if we had used pre-allocated segments. This solution is something to look into if the server is to be run on cheap hardware.
//!
//! #### The cache
//! When developing the program, we simply chose the first CSV library we found on crates.io. We have not had time to compare it to other libraries, so we do not know whether or not it is poorly implemented or not. However after running a quick profile of the program, close to 99 % of the CPU time was spent in the CSV library. To decrease the CPU usage, we implemented a cache instead of reading from the CSV file directly. This was necessary because the server couldn't run in real-time on a laptop. If we would have had more time available we would try different CSV libraries as an alternative to using cache files. This is because using a cache makes the system more complicated than necessary.
//!
//! #### The green thread model
//! We chose to use Tokio with a green threading model mainly because Hyper (HTTP library) is built on top of Tokio. This means if we were to use Hyper, we would need to use Tokio as well. Using Tokio means the server will be able to scale from one to millions of clients with little change in needed CPU power. This is because in a server where all network connections are handled in its own thread spends most of its time switching OS threads. However since we are only going to have a few clients, using a green thread model is a bit overkill if there were no other reasons to use Tokio.
//!
//! One advantage of using Tokio is that each process can be represented by functional programming. This is a very efficient approach to implementing client-server functionality. This means we could pipe data with very little effort. The main disadvantage of using Tokio and futures in Rust is that the syntax is still in its infancy. There is a crate adding macros for using async/await syntax, however since this is still experimental, we decided not to use this. In the future we would have used an async/await syntax to make the code much easier to read.
//!
//!
//! ## Overview over main
//!
//! This module parses command parameters and the config file, and starts all the other modules in their own threads. Finally it joins on all the threads in order to keep the program alive until all modules have stopped. Currently stopping functionality is not implemented. Instead you just kill the program using any code as there is no cleanup to be done.
//!
//! ### More detailed overview
//! The main module reads the command parameters and the config parameters. To set up the server it creates a vector that will contain all threads the main thread should join. Main then starts the HTTP server in a new thread by calling http::Server::new().run(&Addr, CommandTx). It then proceeds to start the TCP server in a similar way. It then extracts a reference to the TCP clients list from the TCP server object. Lastly the Controller is started in a separate thread, and is initialized with the CommandRx and Clients list. In addition it is passed either None or Some(filename). If Some(filename) is specified, the controller will generate a cache from the specified CSV file. If None is specified, the controller will run on the cache.
//!

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

mod tcp;
mod http;
mod controller;

use std::{fs, thread};
use controller::Controller;


/// Struct to represent the config.json-file.
#[derive(Deserialize)]
struct Config {
    /// What port the HTTP server should be listening on.
    http_port: u16,
    /// What port the TCP server should be listening on.
    tcp_port: u16,
}

/// Starts the different threads.
fn main() {
    // Parse the input arguments:
    let mut args = std::env::args();
    let mode = args.nth(1).unwrap();

    let mut controller_input = None;
    let mut run = true;
    match mode.as_str() {
        "run" => controller_input = None,
        "build" => controller_input = Some(args.next().expect("No filename given.")),
        "clear" => {
            run = false;
            for i in 0..60 {
                let _ = fs::remove_file(format!(".{}.dat", i));
            }
        },
        "help" => {
            run = false;
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

    if !run {
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