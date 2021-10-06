use std::{convert::TryInto, time::{ SystemTime }};
use rustls::ServerName;
use crate::transport::Host;

pub fn from_epoch_to(seconds: u64) -> u64 {
    let since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    since_epoch.as_secs() + seconds
}

pub fn as_server_name(host: &Host) -> ServerName {
    (&host.0 as &str).try_into().unwrap()
}