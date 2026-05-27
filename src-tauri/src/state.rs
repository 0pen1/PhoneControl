use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

use crate::adb::server::AdbServer;
use crate::adb::stream::{ControlSockets, StreamTokens};

pub const ADB_PERMITS: u32 = 12;

pub struct AppState {
    pub servers: Arc<Mutex<Vec<AdbServer>>>,
    pub stream_tokens: StreamTokens,
    pub control_sockets: ControlSockets,
    pub adb_semaphore: Arc<Semaphore>,
}

impl AppState {
    pub fn new(servers: Vec<AdbServer>) -> Self {
        Self {
            servers: Arc::new(Mutex::new(servers)),
            stream_tokens: crate::adb::stream::new_tokens(),
            control_sockets: crate::adb::stream::new_control_sockets(),
            adb_semaphore: Arc::new(Semaphore::new(ADB_PERMITS as usize)),
        }
    }
}
