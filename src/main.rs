use irc::client::prelude::*;
use rocket::futures::FutureExt;
use rocket::futures::StreamExt;
use rocket::get;
use rocket::routes;
use rocket::tokio::sync::{mpsc, oneshot};
use rocket::tokio::task;
use std::time::Duration;
use tracing::{error, info, warn};

#[derive(Clone)]
struct IrcClientHandle {
    sender: mpsc::Sender<IrcMessage>,
}

#[derive(Debug)]
enum IrcMessage {
    Send(String, oneshot::Sender<String>),
}

macro_rules! irc_command_handler {
    ($name:ident, $route:expr, $command_template:expr, $response_prefix:expr) => {
        #[get($route)]
        async fn $name(username: String, handle: &rocket::State<IrcClientHandle>) -> String {
            let (response_tx, response_rx) = oneshot::channel();

            let command = format!($command_template, username);

            if handle
                .sender
                .send(IrcMessage::Send(command, response_tx))
                .await
                .is_err()
            {
                return "Failed to send message to IRC client".to_string();
            }

            match tokio::time::timeout(Duration::from_secs(10), response_rx).await {
                Ok(Ok(response)) => {
                    if let Some(r) = response.strip_prefix($response_prefix) {
                        r.to_string()
                    } else {
                        response
                    }
                }
                _ => "Failed to receive response from tildebot".to_string(),
            }
        }
    };
}

irc_command_handler!(time, "/time/<username>", ",time {}", ":[\u{3}03Time\u{3}] ");
irc_command_handler!(
    weather,
    "/weather/<username>",
    ",weather {}",
    ":[\u{3}03Weather\u{3}] "
);

async fn connect_and_run_irc(mut rx: mpsc::Receiver<IrcMessage>) {
    loop {
        let config = Config {
            nickname: Some("tt-t-bot".to_owned()),
            server: Some("irc.tilde.chat".to_owned()),
            port: Some(6697),
            use_tls: Some(true),
            channels: vec![],
            ..Default::default()
        };

        match Client::from_config(config).await {
            Ok(mut client) => {
                if let Err(e) = client.identify() {
                    error!("Failed to identify with IRC server: {:?}", e);
                    continue;
                }

                let mut stream = client.stream().unwrap();

                while let Some(msg) = rx.recv().await {
                    match msg {
                        IrcMessage::Send(irc_message, response_tx) => {
                            // Drain any pending messages
                            while let Some(Some(Ok(msg))) = stream.next().now_or_never() {
                                info!("Received: {:?}", msg);
                            }

                            // Attempt to send message
                            if client.send_privmsg("tildebot", &irc_message).is_ok() {
                                while let Some(Ok(reply)) = stream.next().await {
                                    let rs = reply.to_string();
                                    if let Some(r) = rs.split("tt-t-bot ").last() {
                                        let _ = response_tx.send(r.to_string());
                                        break;
                                    }
                                }
                            } else {
                                let _ = response_tx.send("Failed to send IRC command".to_string());
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to connect to IRC server: {:?}", e);
            }
        }

        warn!("Disconnected from IRC server. Reconnecting in 5 seconds...");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[rocket::main]
async fn main() -> Result<(), rocket::Error> {
    // Set up logging
    tracing_subscriber::fmt().init();

    let (tx, rx) = mpsc::channel::<IrcMessage>(100);
    let irc_handle = IrcClientHandle { sender: tx.clone() };

    // Start IRC task
    task::spawn(connect_and_run_irc(rx));

    // Start Rocket web server
    rocket::build()
        .manage(irc_handle)
        .mount("/", routes![time, weather])
        .launch()
        .await?;

    Ok(())
}
