use std::{collections::HashMap, env, str, sync::Arc};

use tokio::{
    io::{split, AsyncReadExt, AsyncWriteExt},
    net::UdpSocket,
    sync::Mutex,
};
use tokio_serial::SerialPortBuilderExt;

use macaddr::MacAddr6;

#[cfg(unix)]
const DEFAULT_TTY: &str = "/dev/ttyUSB0";
#[cfg(windows)]
const DEFAULT_TTY: &str = "COM8";

#[tokio::main]
async fn main() -> tokio_serial::Result<()> {
    let mut args = env::args();
    let tty_path = args.nth(1).unwrap_or_else(|| DEFAULT_TTY.into());

    let port = tokio_serial::new(tty_path, 921600).open_native_async()?;
    let (mut port_in, port_out) = split(port);
    let port_out = Arc::new(Mutex::new(port_out));

    let mut trackers = HashMap::new();

    loop {
        let mut mac_buffer = [0u8; 6];
        port_in.read_exact(&mut mac_buffer).await?;
        let mac = MacAddr6::from(mac_buffer);
        let length = port_in.read_u16().await?;
        println!("Packet from {mac} of length {length}");

        let mut buffer = vec![0u8; length as usize];
        port_in.read_exact(&mut buffer).await?;
        // println!("{:X?}", buffer);

        let socket = if !trackers.contains_key(&mac) {
            let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
            socket.connect("127.0.0.1:6969").await.unwrap();

            println!("Created new socket");

            trackers.insert(mac, socket.clone());

            let port_out = port_out.clone();

            let socket_thread = socket.clone();
            tokio::spawn(async move {
                loop {
                    let mut buffer = [0u8; 1024];
                    let length = socket_thread.recv(&mut buffer).await.unwrap();

                    println!("Packet to   {mac} of length {length}");

                    let mut port_out = port_out.lock().await;

                    port_out.write_all(mac.as_bytes()).await.unwrap();
                    port_out
                        .write_u16(length.try_into().unwrap())
                        .await
                        .unwrap();
                    port_out.write_all(&buffer[0..length]).await.unwrap();
                    port_out.flush().await.unwrap();

                    // println!("{:X?}", &buffer[0..length]);
                }
            });

            socket
        } else {
            trackers.get(&mac).unwrap().to_owned()
        };

        socket.send(&buffer).await?;
    }
}
