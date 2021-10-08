
use dove::container::*;
use dove::cbs::{SasConfig};
use tokio::time::Duration;

#[tokio::main]
async fn main() {
    let hostname = "<namespace>.servicebus.windows.net";
    let port: u16 = 5671;

    let opts = ConnectionOptions {
        username: None,
        password: None,
        sasl_mechanism: Some(SaslMechanism::Anonymous),
        idle_timeout: Some(Duration::from_secs(10)),
        tls_config: None
    };

    let container = Container::new()
        .expect("unable to create container")
        .start();

    let connection = container
        .connect(hostname, port, opts)
        .await
        .expect("connection not created");

    let session = connection
        .new_session(None)
        .await
        .expect("session not created");
    
    let sender = session
        .with_cbs(SasConfig::new
            ( "<sasl_key_name>"
            , "<sasl_key>"
            , 60 * 60 * 24 * 7
            , "amqps://<namespace>.servicebus.windows.net:5671/<queue>"
            )
        )
        .await
        .new_sender("amqps://<namespace>.servicebus.windows.net:5671/<queue>")
        .await
        .expect("sender not created");

    let _ = sender.send(Message::amqp_value(Value::String(String::from("Hello from Rust.")))).await;
}