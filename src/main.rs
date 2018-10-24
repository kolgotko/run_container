extern crate libjail;
extern crate serde_json;
extern crate clap;
extern crate nix;

use libjail::*;
use nix::unistd::{fork, ForkResult};
use std::error::Error;
use std::process::Command;
use std::collections::HashMap;
use std::thread;
use std::os::unix::net::UnixStream;

struct Container {
    name: String,
}

impl Container {

    fn new() -> Self {

        unimplemented!();

    }

}

fn mount_nullfs(src: impl Into<String>, dst: impl Into<String>, options: Vec<String>) 
    -> Result<(), Box<Error>> {

        unimplemented!();
        let options: Vec<String> = options.into_iter()
            .map(|item| item.into())
            .collect();

        let mut args: Vec<String> = Vec::new();

        if options.len() > 0 {

            args.extend_from_slice(&["-o".into()]);
            args.extend(options);

        }

        args.push(src.into());
        args.push(dst.into());

        let ecode = Command::new("/sbin/mount_nullfs")
            .args(args.as_slice())
            .spawn()?
            .wait()?;

        if let Some(code) = ecode.code() {


        
        }

        Ok(())

    }

fn main() -> Result<(), Box<Error>> {

    let command = "top";
    let programm = std::env::current_exe().unwrap();
    let slave = match std::env::var("SLAVE") {
        Ok(_) => true,
        _ => false,
    };

    if slave { child_main(); }
    else {

        let socket = UnixStream::connect("/tmp/run_container.unix")?;

        thread::spawn(move || -> Result<(), Box<Error + Send + Sync>> {

            Command::new(programm)
                .env("SLAVE", "1")
                .spawn()?
                .wait();

            Ok(())

        });

        parent_main();

    }

    // let result = mount_nullfs("/usr/local/jmaker", "/mnt", vec![]);
    // println!("{:?}", result);


    // match fork()? {
    //     ForkResult::Parent{ child } => {
    //         println!("child pid: {}", child);
    //         parent_main()?; 
    //     },
    //     ForkResult::Child => { child_main()?; },
    // }

    Ok(())

}

fn child_main() -> Result<(), Box<Error>> {

    println!("i am child");

    let mut stream = UnixStream::connect("/tmp/run_container.unix")?;
    let path = "/jails/freebsd112".to_string();

    let mut rules: HashMap<Val, Val> = HashMap::new();
    rules.insert("path".into(), path.into());
    rules.insert("name".into(), "freebsd112".into());
    rules.insert("host.hostname".into(), "freebsd112.service.jmaker".into());
    rules.insert("allow.raw_sockets".into(), true.into());
    rules.insert("allow.socket_af".into(), true.into());
    rules.insert("ip4".into(), JAIL_SYS_INHERIT.into());

    let jid = libjail::set(rules, Action::create() + Modifier::attach())?;

    use std::io::Write;
    stream.write_all(&[jid as u8]);
    println!("jid: {:?}", jid);

    Command::new("top")
        .spawn()?
        .wait();

    libjail::remove(jid)?;

    Ok(())

}

fn parent_main() -> Result<(), Box<Error>> {

    println!("i am parent");
    loop { }
    Ok(())

}
