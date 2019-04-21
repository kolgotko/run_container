extern crate serde;
use serde::{ Deserialize, Serialize };
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug)]
pub struct RunContainerMessage {
    pub name: String,
    pub rootfs: String,
    pub workdir: String,
    pub rules: HashMap<String, String>,
    pub mounts: Vec<HashMap<String, String>>,
    pub interface: String,
    pub entry: String,
    pub command: String,
    pub env: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct StopContainerMessage {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GetTtyMessage {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WaitContainerMessage {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MessageBody<T> {
    pub body: T,
}
