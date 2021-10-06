use std::time::{ SystemTime };

pub fn from_epoch_to(seconds: u64) -> u64 {
    let since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    since_epoch.as_secs() + seconds
}