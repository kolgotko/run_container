extern crate libjail;
extern crate serde_json;
extern crate clap;
extern crate nix;
extern crate signal_hook;

use libjail::*;
use nix::unistd::{fork, ForkResult, close};
use std::error::Error;
use std::process;
use std::collections::HashMap;
use std::thread;
use std::os::unix::net::UnixStream;
use std::os::unix::io::AsRawFd;
use std::net::Shutdown;
use std::io::Read;
use std::io::Write;


fn main() -> Result<(), Box<Error>> {

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
