extern crate libjail;
extern crate serde_json;
extern crate clap;
extern crate nix;
extern crate signal_hook;
extern crate run_container;

use libjail::*;
use libjail::Val as JailValue;
use run_container::AsJailMap;
use nix::unistd::{fork, ForkResult, close, getppid};
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


fn main() -> Result<(), Box<Error>> {

    let container_path = Path::new("/usr/local/jmaker/containers/ac-bt");
    let rootfs = container_path.join("rootfs");
    let manifest = container_path.join("manifest.json");
    let reader = File::open(&manifest)?;
    let json: serde_json::Value = from_reader(reader)?;
    let name = json.get("name").unwrap();
    let json_rules = json.get("rules").unwrap();

    println!("container: {:?}", container_path);
    println!("rootfs: {:?}", rootfs);
    println!("manifest: {:?}", manifest);
    println!("name: {:?}", name);

    let json_map = json_rules.as_object().unwrap();
    let jail_map = json_map.as_jail_map()?;
    println!("{:#?}", jail_map);

    // for (key, value) in map.iter() {

    //     println!("key: {:?}, value: {:?}", key, value);

    // }

    panic!();


    println!("mounts()");
    let mut rules: HashMap<Val, Val> = HashMap::new();
    rules.insert("path".into(), "/jails/freebsd112".into());
    rules.insert("name".into(), "freebsd112".into());
    rules.insert("host.hostname".into(), "freebsd112.service.jmaker".into());
    rules.insert("allow.raw_sockets".into(), true.into());
    rules.insert("allow.socket_af".into(), true.into());
    rules.insert("ip4".into(), JAIL_SYS_INHERIT.into());
    rules.insert("persist".into(), true.into());

    let (mut master, mut slave) = UnixStream::pair()?;

    println!("persist_jail()");
    let jid = libjail::set(rules, Action::create())?;
    println!("create_child[fork()]()");

    let sig_int_id = unsafe { 
        signal_hook::register(signal_hook::SIGINT, move || {
            libjail::remove(jid); 
            process::abort();
        })
    }?;

    let sig_term_id = unsafe { 
        signal_hook::register(signal_hook::SIGTERM, move || {
            libjail::remove(jid); 
            process::abort();
        })
    }?;

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
            process::Command::new("ping")
                .args(&["ya.ru"])
                .spawn()?
                .wait()?;

            println!("slave_exit()");

        },
    }

    Ok(())

}
