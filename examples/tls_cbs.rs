
use std::convert::TryInto;
use dove::cbs::SasConfig;
use dove::container::*;
use dove::transport::TlsConfig;
use rustls::OwnedTrustAnchor;
use rustls::RootCertStore;
use tokio::time::Duration;

#[tokio::main]
async fn main() {
    let hostname = "<namespace>.servicebus.windows.net";
    let port: u16 = 5671;

    // create a ClientConfig, by whatever means you wish
    let mut root_store = RootCertStore::empty();
            root_store.add_server_trust_anchors(
                webpki_roots::TLS_SERVER_ROOTS
                    .0
                    .iter()
                    .map(|ta| {
                        OwnedTrustAnchor::from_subject_spki_name_constraints(
                            ta.subject,
                            ta.spki,
                            ta.name_constraints,
                        )
                    }),
            );
    
    let config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    // If tls_config is Some, it will attempt to create a tls connection
    let opts = ConnectionOptions {
        username: None,
        password: None,
        sasl_mechanism: Some(SaslMechanism::Anonymous),
        idle_timeout: Some(Duration::from_secs(10)),
        tls_config: Some(TlsConfig {
            config: config,
            server_name: hostname.try_into().unwrap()
        })
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

    let sender_new = session
        .with_cbs(SasConfig::new(
            "RootManageSharedAccessKey",
            "blahblah=",
            60 * 60 * 24 * 7,
            "amqps://<namespace>.servicebus.windows.net:5671/<queue>"
        ))
        .await
        .new_sender("amqps://<namespace>:5671/<queue>")
        .await
        .unwrap();

    sender_new.send(Message::amqp_value(Value::String(String::from("Hello from Rust.")))).await.unwrap();
}