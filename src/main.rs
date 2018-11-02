extern crate libjail;
extern crate serde_json;
extern crate clap;
extern crate nix;
extern crate signal_hook;
extern crate run_container;
extern crate lazy_static;
extern crate jsonrpc_core;

use libjail::*;
use libjail::Val as JailValue;
use run_container::AsJailMap;
use nix::unistd::{fork, ForkResult, close, getppid};
use lazy_static::lazy_static;
use std::error::Error;
use std::process;
use std::collections::HashMap;
use std::thread;
use std::os::unix::net::UnixStream;
use std::os::unix::io::AsRawFd;
use std::net::Shutdown;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::net::{Ipv4Addr, Ipv6Addr};

use std::fs::File;
use serde_json::from_reader;
use serde_json::Value as JsonValue;

use std::sync::{Arc, Mutex};

lazy_static! {

    static ref WORKING_JAILS: Arc<Mutex<Vec<i32>>> = {

        let vec: Vec<i32> = Vec::new();
        let mutex = Mutex::new(vec);
        let arc = Arc::new(mutex);
        arc

    };

}

fn process_abort() {

    let ref_jails = WORKING_JAILS.clone();
    let mut jails = ref_jails.lock().unwrap();

    for jid in jails.drain(0..) {
        libjail::remove(jid); 
    }

    process::abort();

}


fn main() -> Result<(), Box<Error>> {

    let (int, term) = unsafe {
        let int = signal_hook::register(signal_hook::SIGINT, process_abort)?;
        let term = signal_hook::register(signal_hook::SIGTERM, process_abort)?;
        (int, term)
    };

    let child0 = thread::spawn(move || {

        run("/usr/local/jmaker/containers/ac-bt".into());

    });

    let child1 = thread::spawn(move || {

        run("/usr/local/jmaker/containers/freebsd112".into());

    });


    child0.join();
    child1.join();


    Ok(())

}

fn run(container_path: String) -> Result<(), Box<Error>> {

    let container_path = Path::new(&container_path);
    let rootfs = container_path.join("rootfs").to_string_lossy().to_string();
    let manifest = container_path.join("manifest.json");
    let reader = File::open(&manifest)?;
    let json: serde_json::Value = from_reader(reader)?;
    let name = json.get("name").unwrap();
    let name = name.as_str().ok_or("name conversion error")?;
    let json_rules = json.get("rules").unwrap();

    println!("container: {:?}", container_path);
    println!("rootfs: {:?}", rootfs);
    println!("manifest: {:?}", manifest);
    println!("name: {:?}", name);

    let json_map = json_rules.as_object().unwrap();
    let mut jail_map = json_map.as_jail_map()?;
    jail_map.insert("path".into(), rootfs.into());
    jail_map.insert("name".into(), name.into());
    jail_map.insert("persist".into(), true.into());

    // jail_map.remove(&"ip4.addr".into());
    // jail_map.remove(&"ip6.addr".into());

    let mut rules = jail_map;

    println!("{:#?}", rules);
    let (mut master, mut slave) = UnixStream::pair()?;

    println!("persist_jail()");
    let jid = libjail::set(rules, Action::create())?;
    println!("create_child[fork()]()");

    {
        let ref_jails = WORKING_JAILS.clone();
        let mut jails = ref_jails.lock().unwrap();
        jails.push(jid);
    }

    match fork()? {
        ForkResult::Parent{ child } => {

            close(slave.as_raw_fd())?;

            println!("child pid: {}", child);
            let mut buffer: Vec<u8> = Vec::new();
            master.read_to_end(&mut buffer)?;

            libjail::remove(jid)?;
            println!("umounts()");
            println!("master_exit()");

        },
        ForkResult::Child => {

            close(master.as_raw_fd())?;

            libjail::attach(jid)?;
            process::Command::new("gstat")
                // .args(&["ya.ru"])
                .spawn()?
                .wait()?;

            println!("slave_exit()");

        },
    }

    Ok(())

}
