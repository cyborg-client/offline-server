//! Pipes data from the CSV file to the TCP clients.

use byteorder::{BigEndian, ByteOrder};
use bytes::{BufMut, Bytes, BytesMut};
use csv::ReaderBuilder;
use futures::sync::oneshot;
use std;
use std::{fs, thread};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::time::{Duration, Instant};
use tcp::Clients;

/// Creates a Duration object from microseconds.
fn duration_from_micros(micros: u64) -> Duration {
    Duration::new(
        micros / 1_000_000,
        ((micros % 1_000_000) as u32) * 1_000,
    )
}

/// Represents the server state and is configured using the POST /start.
#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    /// The sample rate when sampling the MEA.
    pub sample_rate: u32,
    /// The number of samples to send for each channel each time.
    pub segment_length: u32
}

/// Represents a running state.
pub type Running = bool;

/// The command to send on the Command channel.
pub enum Command {
    /// Start the server with the specified Config.
    Start(Config),
    /// Stop the server.
    Stop
}

/// The type for the sending side of the Command channel.
///
/// This channel sends a tuple containing the Command and a oneshot channel for acknowledging the command.
pub type CommandTx = std::sync::mpsc::Sender<(Command, oneshot::Sender<()>)>;
/// The type for the receiving side of the Command channel.
///
/// This channel receives a tuple containing the Command and a oneshot channel for acknowledging the command.
pub type CommandRx = std::sync::mpsc::Receiver<(Command, oneshot::Sender<()>)>;

/// Represents a single line in the CSV file.
#[derive(Deserialize)]
struct Sample {
    /// The timestamp value.
    #[allow(unused)]
    timestamp: u64,
    /// The 60 voltage values.
    values: Vec<i32>,
}

pub struct Controller {
    command_rx: CommandRx,
    clients: ::tcp::Clients,
    config: Option<Config>,
    samples: Vec<BufReader<fs::File>>,
    last_segment_finished: Instant,
}

impl Controller {
    pub fn new(command_rx: CommandRx, clients: Clients, filename: Option<String>) -> Controller {
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

                println!("Done building cache. Run with 'run'.");
                std::process::exit(0);
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
        let config;
        {
            let config_clone = self.config.clone();
            match config_clone {
                Some(ref val) => {
                    config = val.clone()
                },
                None => {
                    panic!("Config not set.");
                }
            }
        }

        let mut result = BytesMut::with_capacity(config.segment_length as usize * std::mem::size_of::<i32>() * 60).writer();

        let mut bytes_buf_vec = vec![0u8; config.segment_length as usize * std::mem::size_of::<i32>()];
        let mut reset_files = false;
        for file in &mut self.samples {
            let mut bytes_buf = bytes_buf_vec.as_mut_slice();
            match file.read_exact(&mut bytes_buf) {
                Ok(_) => {},
                Err(_) => {
                    reset_files = true;
                    println!("Resetting files.");
                    break;
                }
            }
            result.write_all(&mut bytes_buf).unwrap();
        }

        if reset_files {
            for file in &mut self.samples {
                file.seek(SeekFrom::Start(0)).unwrap();
            }
            return self.collect_segment();
        }

        let result = result.into_inner().freeze();


        self.last_segment_finished += duration_from_micros((config.segment_length * 1000000 / config.sample_rate) as u64);
        self.sleep_until(self.last_segment_finished);
        result
    }

    pub fn run(mut self) {
        loop {
            self.update_config();

            let segment = self.collect_segment();

            let mut broken_clients = Vec::new();
            let mut clients = self.clients.lock().unwrap();
            for (address, tx) in clients.iter_mut() {
                if let Err(_) = tx.try_send(segment.clone()) {
                    println!("Killing {} because it is lagging too far behind.", address);
                    broken_clients.push(address.clone());
                }
            }

            for i in broken_clients.iter().rev() {
                clients.remove(i);
            }
        }
    }
}