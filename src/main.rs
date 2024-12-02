use irc::client::prelude::*;
use rocket::futures::FutureExt;
use rocket::futures::StreamExt;
use rocket::get;
use rocket::routes;
use rocket::tokio::sync::{mpsc, oneshot};
use rocket::tokio::task;
use std::time::Duration;

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

            // Format the command using the provided template
            let command = format!($command_template, username);

            // Send the command to the IRC client
            if handle
                .sender
                .send(IrcMessage::Send(command, response_tx))
                .await
                .is_err()
            {
                return "Failed to send message to IRC client".to_string();
            }

            // Wait for and process the response
            match tokio::time::timeout(Duration::from_secs(10), response_rx).await {
                Ok(Ok(response)) => {
                    // Strip the prefix if it exists
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

#[rocket::main]
async fn main() -> Result<(), rocket::Error> {
    let (tx, mut rx) = mpsc::channel::<IrcMessage>(100);
    let irc_handle = IrcClientHandle { sender: tx.clone() };

    // Spawn a task to handle IRC communications
    task::spawn(async move {
        let config = Config {
            nickname: Some("tt-t-bot".to_owned()),
            server: Some("irc.tilde.chat".to_owned()),
            port: Some(6697),
            use_tls: Some(true),
            channels: vec![],
            ..Default::default()
        };

        // Create the client
        let mut client = Client::from_config(config).await.unwrap();
        client.identify().unwrap();

        // Get a stream of messages
        let mut stream = client.stream().unwrap();

        while let Some(msg) = rx.recv().await {
            match msg {
                IrcMessage::Send(irc_message, response_tx) => {
                    // Drain any existing messages in the stream first
                    while let Some(Some(Ok(msg))) = stream.next().now_or_never() {
                        println!("{}", msg)
                    }
                    println!("End.");

                    // Send the message
                    if client.send_privmsg("tildebot", &irc_message).is_ok() {
                        // Wait for and capture the specific reply
                        while let Some(Ok(reply)) = stream.next().await {
                            // Check if the reply is relevant
                            let rs = reply.to_string();

                            println!("{:#?}", rs);
                            let r = if let Some(r) = rs.split("tt-t-bot ").last() {
                                r
                            } else {
                                "Failed to receive reply"
                            };
                            let _ = response_tx.send(r.to_string());
                            break;
                        }
                    } else {
                        let _ = response_tx.send("Failed to send IRC command".to_string());
                    }
                }
            }
        }
    });

    rocket::build()
        .manage(irc_handle)
        .mount("/", routes![time, weather])
        .launch()
        .await?;

    Ok(())
}
