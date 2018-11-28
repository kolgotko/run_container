extern crate libjail;
extern crate libmount;
extern crate serde_json;
extern crate clap;
extern crate nix;
extern crate signal_hook;
extern crate lazy_static;
extern crate jsonrpc_core;
extern crate command_pattern;
extern crate forkpty;
extern crate uuid;
extern crate libc;

mod as_jail_map;
use self::as_jail_map::AsJailMap;

mod path_macros;
use self::path_macros::*;

use std::env;
use std::fs;
use std::ffi::CString;
use std::any::Any;
use libmount::*;
use libjail::*;
use command_pattern::*;
use libjail::Val as JailValue;
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
use serde_json::json;
use serde_json::from_reader;
use serde_json::Value as JsonValue;

use std::sync::{Arc, Mutex};

use jsonrpc_core::IoHandler as RpcHandler;
use jsonrpc_core::Params as RpcParams;
use jsonrpc_core::Value as RpcValue;
use jsonrpc_core::Error as RpcError;
use jsonrpc_core::ErrorCode as RpcErrorCode;

use forkpty::*;

use uuid::Uuid;


const SOCKET_FILE: &str = "/tmp/run_container.sock";

type AnyInvoker = Invoker<Box<dyn Any>>;

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

    static ref NAMED_TASKS: Mutex<HashMap<String, Box<fn() -> ()> >> = {

        let map: HashMap<String, Box<fn() -> ()>> = HashMap::new();
        let mutex = Mutex::new(map);
        mutex

    };

    static ref NAMED_TTY_SESSIONS: Mutex<HashMap<String, PtyMaster>> = {

        let map: HashMap<String, PtyMaster> = HashMap::new();
        let mutex = Mutex::new(map);
        mutex

    };

    static ref RPC_HANDLER: RpcHandler = {

        let mut rpc_handler = RpcHandler::new();
        rpc_handler.add_method("run_container", run_container);
        rpc_handler.add_method("stop_container", stop_container);
        rpc_handler.add_method("get_tty", get_tty);
        rpc_handler

    };

}

fn process_abort() {

    fs::remove_file(SOCKET_FILE);

    let mut named_invokers = NAMED_INVOKERS
        .lock()
        .unwrap();

    for (_, mut invoker) in named_invokers.drain() {

        invoker.undo_all();

    }

    process::abort();

}


fn get_tty(params: RpcParams) -> Result<RpcValue, RpcError> {

    let json: JsonValue = params.parse()?;
    println!("params: {:#?}\n", json["body"]);

    let name = &json["body"]["name"].as_str().unwrap().to_string();

    let (mut out_tty, mut in_tty) = {

        let tty_sessions = NAMED_TTY_SESSIONS.lock().unwrap();

        if !tty_sessions.contains_key(name) {
            let mut error = RpcError::new(RpcErrorCode::ServerError(-404));
            error.message = "tty session not found".to_string();
            return Err(error);
        }

        let tty = tty_sessions.get(name).unwrap();

        (tty.clone(), tty.clone())

    };

    let id = Uuid::new_v4();
    let id = id.to_hyphenated().to_string();
    let tmp_dir = path_join!(env::temp_dir(), &id);
    fs::create_dir_all(&tmp_dir).unwrap();

    let input_path = path_join!(&tmp_dir, "in.sock");
    let output_path = path_join!(&tmp_dir, "out.sock");

    let io_paths = (input_path.clone(), output_path.clone());

    thread::spawn(move || {

        let (input_path, output_path) = io_paths;

        let out_listener = UnixListener::bind(&output_path).unwrap();
        let in_listener = UnixListener::bind(&input_path).unwrap();

        let out_thread = thread::spawn(move || {

            let (mut out_stream, _) = out_listener.accept().unwrap();
            let mut buffer: Vec<u8> = vec![0; libc::BUFSIZ as usize];

            loop {
                let count = out_tty.read(&mut buffer).unwrap();
                out_stream.write(&buffer[0..count]).unwrap();
                out_stream.flush().unwrap();
            }

        });

        let in_thread = thread::spawn(move || {

            let (mut in_stream, _) = in_listener.accept().unwrap();

            for byte in in_stream.bytes() {
                in_tty.write(&[byte.unwrap()]);
            }

        });

        out_thread.join();
        in_thread.join();

        println!("close remote tty session");

    });

    Ok(json!({
        "input": input_path,
        "output": output_path
    }))
}

fn stop_container(params: RpcParams) -> Result<RpcValue, RpcError> {

    let json: JsonValue = params.parse()?;
    let name = &json["body"]["name"].as_str().unwrap();

    {
        let mut named_invokers = NAMED_INVOKERS
            .lock()
            .unwrap();

        let invoker = named_invokers
            .remove(&name.to_string());

        match invoker {
            Some(mut invoker) => {
                invoker.undo_all();
                Ok(json!(true))
            },
            None => {
                let mut error = RpcError::new(RpcErrorCode::ServerError(-404));
                error.message = "container not found".to_string();
                Err(error)
            }
        }

    }

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
    let workdir = &json["body"]["workdir"].as_str().unwrap_or("/");
    let interface = &json["body"]["interface"].as_str().unwrap_or("");
    let entry = &json["body"]["entry"].as_str().unwrap_or("");
    let command = &json["body"]["command"].as_str().unwrap_or("");
    let command = format!("{} {}", entry, command);
    let envs = serde_json::Map::new();
    let envs = &json["body"]["env"]
        .as_object()
        .unwrap_or(&envs);

    let mut jail_map = rules.as_jail_map().unwrap();
    jail_map.insert("path".into(), rootfs.to_owned().into());
    jail_map.insert("name".into(), name.to_owned().into());
    jail_map.insert("persist".into(), true.into());

    println!("{:#?}", jail_map);

    println!("mounts!");

    let devfs = path_join!(rootfs, "/dev");
    let devfs = devfs.to_str().unwrap().to_owned();
    let fdescfs = path_join!(rootfs, "/dev/fd");
    let fdescfs = fdescfs.to_str().unwrap().to_owned();
    let procfs = path_join!(rootfs, "/proc");
    let procfs = procfs.to_str().unwrap().to_owned();

    let for_exec = (devfs, fdescfs, procfs);
    let for_unexec = for_exec.clone();

    exec_or_undo_all!(invoker, {
        exec: move {

            let (devfs, fdescfs, procfs) = for_exec.clone();

            mount_devfs(devfs, mount_options!({ "ruleset" => "4" }), None)?;
            mount_fdescfs(fdescfs, None, None)?;
            mount_procfs(procfs, None, None)?;

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
            .unwrap();

        let dst = path_resolve!(dst).unwrap();
        let dst = path_join!(rootfs, &dst);
        let dst = dst.to_str().unwrap().to_owned();
        let for_exec = (src, dst);
        let for_unexec = for_exec.clone();

        exec_or_undo_all!(invoker, {
            exec: move {

                let (src, dst) = for_exec.clone();

                fs::create_dir_all(&dst)?;
                mount_nullfs(src.to_owned(), dst.to_owned(), None, None)?;
                Ok(Box::new(()) as Box<dyn Any>)

            },
            unexec: move {

                let (src, dst) = for_unexec.clone();
                unmount(dst.to_owned(), Some(libc_mount::MNT_FORCE as i32))?;
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

    {
        let mut jails = WORKING_JAILS.lock().unwrap();
        jails.push(jid);
    }

    let vnet = if let Some(value) = rules.get("vnet") {
        value.as_str().unwrap_or("disabled")
    } else { "disabled" };

    let exec_if = interface.to_string();
    let unexec_if = interface.to_string();

    if vnet == "new" && interface != &"" {

        exec_or_undo_all!(invoker, {
            exec: move {

                process::Command::new("ifconfig")
                    .args(&[&exec_if, "name", "eth0"])
                    .spawn()?;

                process::Command::new("ifconfig")
                    .args(&["eth0", "vnet", &jid.to_string()])
                    .spawn()?;

                Ok(Box::new(()) as Box<dyn Any>)
            },
            unexec: move {

                process::Command::new("ifconfig")
                    .args(&["eth0", "-vnet", &jid.to_string()])
                    .spawn();

                process::Command::new("ifconfig")
                    .args(&["eth0", "name", &unexec_if])
                    .spawn()?;

                Ok(())
            }
        }).unwrap();

    }

    println!("create_child[fork()]()");
    let fork_result = exec_or_undo_all!(invoker, {

        let result = forkpty()?;
        Ok(Box::new(result) as Box<Any>)

    }).unwrap();

    let fork_result = fork_result.downcast_ref::<ForkPtyResult>()
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
        ForkPtyResult::Parent(child, pty_master) => {

            {
                let mut tty_sessions = NAMED_TTY_SESSIONS.lock().unwrap();
                tty_sessions.insert(name.to_string(), pty_master.clone());
            }

            let child = child.clone();
            let name = name.to_string();

            thread::spawn(move || {

                child.wait();

                let mut named_invokers = NAMED_INVOKERS
                    .lock()
                    .unwrap();

                let invoker = named_invokers
                    .remove(&name);

                if let Some(mut invoker) = invoker {
                    invoker.undo_all();
                }

                {
                    let mut tty_sessions = NAMED_TTY_SESSIONS.lock().unwrap();
                    tty_sessions.remove(&name);
                }


            });

        },
        ForkPtyResult::Child(pid) => {

            libjail::attach(jid).unwrap();

            fs::create_dir_all(workdir).unwrap();
            env::set_current_dir(workdir).unwrap();

            for (key, value) in envs.iter() {

                let value = value.as_str().unwrap();
                env::set_var(key, value);

            }

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
