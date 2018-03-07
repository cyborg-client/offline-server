use byteorder::{BigEndian, ByteOrder};
use bytes::{BufMut, Bytes, BytesMut};
use csv::ReaderBuilder;
use futures::sync::oneshot;
use std;
use std::{fs, thread};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::time::{Duration, Instant};
use tcp::Clients;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub sample_rate: u32,
    pub segment_length: u32
}

pub type Running = bool;

pub enum Command {
    Start(Config),
    Stop
}

pub type CommandTx = std::sync::mpsc::Sender<(Command, oneshot::Sender<()>)>;
pub type CommandRx = std::sync::mpsc::Receiver<(Command, oneshot::Sender<()>)>;

#[derive(Deserialize)]
struct Sample {
    #[allow(unused)]
    timestamp: u64,
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
            file.read_exact(&mut bytes_buf).unwrap();
            result.write_all(&mut bytes_buf).unwrap();
        }

        let result = result.into_inner().freeze();


        self.last_segment_finished += Duration::from_micros((config.segment_length * 1000000 / config.sample_rate) as u64);
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
                    broken_clients.push(address.clone());
                }
            }

            for i in broken_clients.iter().rev() {
                clients.remove(i);
            }
        }
    }
}