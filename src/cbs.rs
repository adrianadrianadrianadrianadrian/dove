use crate::container::*;
use crate::utils::from_epoch_to;
use sha2::Sha256;
use hmac::{Hmac, Mac, NewMac};
type HMAC256 = Hmac<Sha256>;
use crate::error::*;

pub struct SasConfig {
    pub sasl_key_name: String,
    pub sasl_key: String,
    pub token_expiry: u64,
    pub address: String
}

pub enum CbsOptions {
    Sas(SasConfig)
}

impl SasConfig {
    pub fn new(
        sasl_key_name: &str, 
        sasl_key: &str, 
        token_expiry: u64, 
        address: &str,
    ) -> CbsOptions {
        CbsOptions::Sas(
            SasConfig 
            { sasl_key_name: String::from(sasl_key_name)
            , sasl_key: String::from(sasl_key)
            , token_expiry
            , address: String::from(address)
            })
    }
}

pub async fn put_sas_token(
    session: &Session, 
    resource_uri: &str, 
    sasl_key_name: &str, 
    sasl_key: &str, 
    expiry_seconds: u64
) -> Result<()>
{
    let token = create_sas_token(resource_uri, sasl_key_name, sasl_key, expiry_seconds);
    let sender = session.new_sender("$cbs").await?;
    let receiver = session.new_receiver("$cbs").await?;

    let mut msg_props =  MessageProperties::new();
    msg_props.message_id = Some(Value::String(String::from("1")));
    msg_props.reply_to = Some(String::from("cbs"));
    let mut message = Message::amqp_value(Value::String(token));
    message.properties = Some(msg_props);
    message.application_properties = Some(
                vec![ (Value::String(String::from("operation")), Value::String(String::from("put-token")))
                    , (Value::String(String::from("type")), Value::String(String::from("servicebus.windows.net:sastoken")))
                    , (Value::String(String::from("name")), Value::String(String::from(resource_uri)))
                    ]);

    sender.send(message).await?;
    let delivery = receiver.receive().await?;

    match &delivery.message().application_properties {
        Some(props) => {
            let success = props.iter().any(|(key, val)| {
                *key == Value::String(String::from("status-code")) 
                    && (*val == Value::Int(202) || *val == Value::Int(200))
            });

            match success {
                true => (),
                false => panic!("put-token failed. Server responded with {:?}.", props)
            }
        },
        None => panic!("put-token failed. Server responded with no application properties.")
    }
    // Use amqpErrors above..

    sender.close(None)?;
    receiver.close(None)?;
    Ok(())
}

fn create_sas_token(
    resource_uri: &str, 
    sasl_key_name: &str, 
    sasl_key: &str,
    expiry_seconds: u64
) -> String
{
    let expiry = from_epoch_to(expiry_seconds);
    let encoded_uri = urlencoding::encode(resource_uri);
    let mut string_to_sign = String::new();
    string_to_sign.push_str(&encoded_uri);
    string_to_sign.push_str("\n");
    string_to_sign.push_str(&expiry.to_string());

    let mut mac = HMAC256::new_from_slice(sasl_key.as_bytes()).unwrap();
    mac.update(string_to_sign.as_bytes());
    let signed_bytes = mac.finalize().into_bytes();
    
    format!( "SharedAccessSignature sr={}&sig={}&se={}&skn={}"
           , urlencoding::encode(resource_uri)
           , urlencoding::encode(&base64::encode(signed_bytes))
           , expiry.to_string()
           , sasl_key_name)
}