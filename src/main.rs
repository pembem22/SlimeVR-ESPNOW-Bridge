use std::{
    collections::HashMap,
    env, str,
    sync::Arc,
    time::{Duration, SystemTime},
    sync::Mutex as StdMutex,
};

use tokio::{
    io::{split, AsyncReadExt, AsyncWriteExt},
    net::UdpSocket,
    sync::Mutex,
    task::JoinHandle,
};
use tokio_serial::SerialPortBuilderExt;

use macaddr::MacAddr6;

#[cfg(unix)]
const DEFAULT_TTY: &str = "/dev/ttyUSB0";
#[cfg(windows)]
const DEFAULT_TTY: &str = "COM5";

struct Tracker {
    socket: Arc<UdpSocket>,
    last_message: SystemTime,
    task: JoinHandle<()>,
}

#[tokio::main]
async fn main() -> tokio_serial::Result<()> {
    let mut args = env::args();
    let tty_path = args.nth(1).unwrap_or_else(|| DEFAULT_TTY.into());

    let port = tokio_serial::new(tty_path, 115200).open_native_async()?;
    let (mut port_in, port_out) = split(port);
    let port_out = Arc::new(Mutex::new(port_out));

    let trackers_mutex: Arc<Mutex<HashMap<MacAddr6, Arc<StdMutex<Tracker>>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let task_trackers_mutex = trackers_mutex.clone();
    tokio::spawn(async move {
        loop {
            let mut trackers = task_trackers_mutex.lock().await;
            trackers.retain(|mac, tracker_mutex| {
                let tracker = tracker_mutex.lock().unwrap();
                let timed_out = SystemTime::now()
                    .duration_since(tracker.last_message)
                    .unwrap()
                    >= Duration::from_secs(1);

                if timed_out {
                    println!("{mac} timed out");
                    tracker.task.abort();
                }

                !timed_out
            });
            drop(trackers);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    loop {
        let mut mac_buffer = [0u8; 6];
        port_in.read_exact(&mut mac_buffer).await?;
        let mac = MacAddr6::from(mac_buffer);
        let length = port_in.read_u16().await?;
        let timestamp = port_in.read_u32().await?;
        println!("{:#08} Packet from {mac} of length {length}", timestamp);

        let mut buffer = vec![0u8; length as usize];
        port_in.read_exact(&mut buffer).await?;
        println!("{:X?}", buffer);

        let mut trackers = trackers_mutex.lock().await;

        let tracker_mutex = if !trackers.contains_key(&mac) {
            let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
            socket.connect("127.0.0.1:6969").await?;

            let task_socket = socket.clone();
            let port_out = port_out.clone();
            let future = async move {
                loop {
                    let mut buffer = [0u8; 1024];
                    let length = task_socket.recv(&mut buffer).await.unwrap();

                    println!("Packet to   {mac} of length {length}");

                    let mut port_out = port_out.lock().await;

                    let mut out_buffer: Vec<u8> = Vec::with_capacity(6 + 2 + length);
                    out_buffer.extend_from_slice(mac.as_bytes());
                    out_buffer.extend_from_slice(&u16::try_from(length).unwrap().to_be_bytes());
                    out_buffer.extend_from_slice(&buffer[0..length]);

                    port_out.write_all(&out_buffer).await.unwrap();
                    port_out.flush().await.unwrap();

                    // println!("{:X?}", &buffer[0..length]);
                }
            };

            let tracker = Arc::new(StdMutex::new(Tracker {
                socket,
                last_message: SystemTime::now(),
                task: tokio::spawn(future),
            }));

            trackers.insert(mac, tracker.clone());
            println!("Created new socket");

            tracker
        } else {
            trackers.get(&mac).unwrap().to_owned()
        };

        let mut tracker = tracker_mutex.lock().unwrap();

        tracker.last_message = SystemTime::now();
        tracker.socket.send(&buffer).await?;
    }
}
