extern crate libjail;
extern crate libmount;
extern crate serde_json;
extern crate clap;
extern crate nix;
extern crate signal_hook;
extern crate run_container;
extern crate lazy_static;
extern crate jsonrpc_core;
extern crate command_pattern;

use std::fs;
use std::ffi::CString;
use std::any::Any;
use libmount::*;
use libjail::*;
use command_pattern::*;
use libjail::Val as JailValue;
use run_container::AsJailMap;
use nix::unistd::{fork, ForkResult, close, getppid, execvp};
use nix::sys::wait::waitpid;
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
use std::os::unix::net::{UnixListener};

use std::fs::File;
use serde_json::from_reader;
use serde_json::Value as JsonValue;

use std::sync::{Arc, Mutex};

use jsonrpc_core::IoHandler as RpcHandler;
use jsonrpc_core::Params as RpcParams;
use jsonrpc_core::Value as RpcValue;
use jsonrpc_core::Error as RpcError;


const SOCKET_FILE: &str = "/tmp/run_container.sock";

type AnyInvoker = Invoker<Box<dyn Any>, Box<dyn Error>>;

lazy_static! {

    static ref WORKING_JAILS: Mutex<Vec<i32>> = {

        let vec: Vec<i32> = Vec::new();
        let mutex = Mutex::new(vec);
        mutex

    };

    static ref NAMED_INVOKERS: Mutex<HashMap<String, AnyInvoker>> = {

        let map: HashMap<String, AnyInvoker> = HashMap::new();
        let mutex = Mutex::new(map);
        mutex

    };

    static ref RPC_HANDLER: RpcHandler = {

        let mut rpc_handler = RpcHandler::new();
        rpc_handler.add_method("run_container", run_container);
        rpc_handler.add_method("stop_container", stop_container);
        rpc_handler

    };

}

fn process_abort() {

    fs::remove_file(SOCKET_FILE);

    let mut named_invokers = NAMED_INVOKERS
        .lock()
        .unwrap();

    for (key, mut invoker) in named_invokers.drain() {

        invoker.undo_all();

    }

    process::abort();

}



fn stop_container(params: RpcParams) -> Result<RpcValue, RpcError> {


    unimplemented!();

    Ok(RpcValue::Null)

}

fn run_container(params: RpcParams) -> Result<RpcValue, RpcError> {

    let mut invoker: AnyInvoker = Invoker::new();
    let json: JsonValue = params.parse()?;
    println!("params: {:#?}\n", json["body"]);

    let path = &json["body"]["path"].as_str().unwrap();
    let rootfs = &json["body"]["rootfs"].as_str().unwrap();
    let rootfs_path = Path::new(&rootfs);
    let name = &json["body"]["name"].as_str().unwrap();
    let rules = &json["body"]["rules"].as_object().unwrap();
    let mounts = &json["body"]["mounts"].as_array().unwrap();
    let entry = &json["body"]["entry"].as_str().unwrap_or("");
    let command = &json["body"]["command"].as_str().unwrap_or("");
    let command = format!("{} {}", entry, command);

    let mut jail_map = rules.as_jail_map().unwrap();
    jail_map.insert("path".into(), rootfs.to_owned().into());
    jail_map.insert("name".into(), name.to_owned().into());
    jail_map.insert("persist".into(), true.into());

    println!("{:?}", jail_map);

    println!("mounts!");

    let devfs = rootfs_path.join("/dev");
    println!("{:?}", devfs);
    let devfs = devfs.to_str().unwrap().to_owned();
    let fdescfs = rootfs_path.join("/dev/fd");
    let fdescfs = fdescfs.to_str().unwrap().to_owned();
    let procfs = rootfs_path.join("/proc");
    let procfs = procfs.to_str().unwrap().to_owned();

    let for_exec = (devfs, fdescfs, procfs);
    let for_unexec = for_exec.clone();

    exec_or_undo_all!(invoker, {
        exec: move {

            let (devfs, fdescfs, procfs) = for_exec.clone();

            mount_devfs(devfs, None)?;
            mount_fdescfs(fdescfs, None)?;
            mount_procfs(procfs, None)?;

            Ok(Box::new(()) as Box<dyn Any>)
        },
        unexec: move {

            let (devfs, fdescfs, procfs) = for_unexec.clone();

            unmount(procfs, Some(libc_mount::MNT_FORCE as i32))?;
            unmount(fdescfs, Some(libc_mount::MNT_FORCE as i32))?;
            unmount(devfs, Some(libc_mount::MNT_FORCE as i32))?;

            Ok(())
        }
    }).unwrap();

    for rule_mount in mounts.iter() {

        let rule_mount = rule_mount.as_object().unwrap();
        let src = rule_mount.get("src")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let dst = rule_mount.get("dst")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let dst = rootfs_path.join(dst);
        let dst = dst.to_str().unwrap().to_owned();
        let for_exec = (src, dst);
        let for_unexec = for_exec.clone();

        exec_or_undo_all!(invoker, {
            exec: move {

                let (src, dst) = for_exec.clone();

                mount_nullfs(src.to_owned(), dst.to_owned(), None)?;
                Ok(Box::new(()) as Box<dyn Any>)

            },
            unexec: move {

                let (src, dst) = for_unexec.clone();
                unmount(dst.to_owned(), None)?;
                Ok(())

            }
        });

    }

    println!("persist_jail()");
    let jail_name = name.to_string();
    let jid = exec_or_undo_all!(invoker, {
        exec: move {

            let jid = libjail::set(jail_map.to_owned(), Action::create())?;
            Ok(Box::new(jid) as Box<Any>)

        },
        unexec: move {

            let rules = libjail::get_rules(jail_name.to_owned(), vec!["jid"])?;
            let jid = rules.get("jid").ok_or("not found property jid")?;

            if let libjail::OutVal::I32(value) = jid {
                libjail::remove(*value)?;
            }

            Ok(())

        }
    }).unwrap();

    let jid: i32 = jid.downcast_ref::<i32>()
        .ok_or("jid cast error.")
        .unwrap()
        .to_owned();

    println!("create_child[fork()]()");

    {
        let mut jails = WORKING_JAILS.lock().unwrap();
        jails.push(jid);
    }

    let fork_result = exec_or_undo_all!(invoker, {

        let result = fork()?;
        Ok(Box::new(result) as Box<Any>)

    }).unwrap();

    let fork_result: ForkResult = fork_result.downcast_ref::<ForkResult>()
        .ok_or("fork_result cast error.")
        .unwrap()
        .to_owned();

    {
        let mut named_invokers = NAMED_INVOKERS
            .lock()
            .unwrap();

        named_invokers.insert(name.to_string(), invoker);
    }

    match fork_result {
        ForkResult::Parent{ child } => {

            waitpid(child, None);

            let mut named_invokers = NAMED_INVOKERS
                .lock()
                .unwrap();

            let mut invoker = named_invokers.remove(name.to_owned()).unwrap();
            invoker.undo_all();

        },
        ForkResult::Child => {

            libjail::attach(jid).unwrap();

            execvp(&CString::new("/bin/sh").unwrap(), &[
               CString::new("").unwrap(),
               CString::new("-c").unwrap(),
               CString::new(command).unwrap(),
            ])
                .unwrap();

        },
    }

    Ok(RpcValue::Null)

}

fn main() -> Result<(), Box<Error>> {

    let listener = UnixListener::bind(SOCKET_FILE)?;

    let (int, term) = unsafe {
        let int = signal_hook::register(signal_hook::SIGINT, process_abort)?;
        let term = signal_hook::register(signal_hook::SIGTERM, process_abort)?;
        (int, term)
    };

    for stream in listener.incoming() {

        thread::spawn(move || -> Result<(), Box<Error + Send + Sync>> {

            let rpc_handler = &RPC_HANDLER;
            let mut stream = stream?;

            let mut buffer: Vec<u8> = Vec::new();
            stream.read_to_end(&mut buffer);
            let recv_string = String::from_utf8(buffer)?;
            let result = rpc_handler.handle_request_sync(&recv_string).unwrap();

            println!("result: {:?}", result);

            stream.write_all(result.as_bytes())?;

            Ok(())

        });

    }

    Ok(())
}

