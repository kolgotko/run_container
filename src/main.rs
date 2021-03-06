extern crate libjail;
extern crate libmount;
extern crate serde;
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
mod message_body;
use message_body::*;

use std::env;
use std::fs;
use std::ffi::CString;
use std::convert::TryInto;
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
use std::thread::Builder as ThreadBuilder;
use std::os::unix::net::UnixStream;
use std::os::unix::io::AsRawFd;
use std::net::Shutdown;
use std::io;
use std::io::{ Read, Write };
use std::path::Path;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::net::{UnixListener};

use std::fs::File;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::from_reader;
use serde_json::Value as JsonValue;

use std::sync::{Arc, Mutex};
use std::sync::mpsc::*;

use jsonrpc_core::IoHandler as RpcHandler;
use jsonrpc_core::Params as RpcParams;
use jsonrpc_core::Value as RpcValue;
use jsonrpc_core::Error as RpcError;
use jsonrpc_core::ErrorCode as RpcErrorCode;

use forkpty::*;

use uuid::Uuid;

const CNI_CONF_DIR: &str = "/usr/local/etc/cni.config";
const CNI_BIN_DIR: &str = "/usr/local/etc/cni";
const SOCKET_FILE: &str = "/tmp/run_container.sock";

#[derive(Debug)]
enum StopCause {
    Signal(nix::sys::signal::Signal),
    Exited(i32),
    Undefined,
}

impl From<WaitStatus> for StopCause {
    fn from(value: WaitStatus) -> Self {
        match value {
            WaitStatus::Signaled(_, signal, _) => {
                StopCause::Signal(signal)
            },
            WaitStatus::Exited(_, code) => {
                StopCause::Exited(code)
            },
            _ => StopCause::Undefined
        }
    }
}

#[derive(Debug)]
enum Events {
    ContainerStoped(String, StopCause),
    ContainerStarted(String),
}

type AnyInvoker = Invoker<Box<dyn Any>>;

lazy_static! {
    static ref SUBSCRIBERS: Mutex<Vec<Sender<Events>>> = {
        let vec: Vec<_> = Vec::new();
        let mutex = Mutex::new(vec);
        mutex
    };

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

    static ref NAMED_TTY_SESSIONS: Mutex<HashMap<String, Arc<PtyMaster>>> = {
        let map: HashMap<String, Arc<PtyMaster>> = HashMap::new();
        let mutex = Mutex::new(map);
        mutex
    };

    static ref RPC_HANDLER: RpcHandler = {
        let mut rpc_handler = RpcHandler::new();
        rpc_handler.add_method("run_container", run_container);
        rpc_handler.add_method("stop_container", stop_container);
        rpc_handler.add_method("wait_container", wait_container);
        rpc_handler.add_method("get_tty", get_tty);
        rpc_handler
    };
}

fn process_abort() {
    fs::remove_file(SOCKET_FILE);
    let mut named_invokers = NAMED_INVOKERS.lock().unwrap();
    for (_, mut invoker) in named_invokers.drain() {
        invoker.undo_all();
    }
    process::abort();
}

fn wait_container(params: RpcParams) -> Result<RpcValue, RpcError> {
    let message: MessageBody<WaitContainerMessage> = params.parse()?;
    let name = message.body.name;
    let rx = {
        let mut subscribers = SUBSCRIBERS.lock().unwrap();
        let (tx, rx) = channel::<Events>();
        subscribers.push(tx);
        rx
    };

    loop {
        let event = rx.recv().unwrap();
        match event {
            Events::ContainerStoped(container, cause) => {
                if container != name { continue; }
                match cause {
                    StopCause::Signal(signal) => {
                        return Ok(json!({
                            "code": JsonValue::Null,
                            "signal": format!("{:?}", signal)
                        }));
                    },
                    StopCause::Exited(code) => {
                        return Ok(json!({
                            "code": code,
                            "signal": JsonValue::Null
                        }));
                    },
                    StopCause::Undefined => {
                        let mut error = RpcError::new(RpcErrorCode::ServerError(-500));
                        error.message = "stop cause is undefined".to_string();
                        return Err(error);
                    }
                }
            },
            _ => continue,
        }
    }

    Ok(RpcValue::Null)
}

fn get_tty(params: RpcParams) -> Result<RpcValue, RpcError> {
    let message: MessageBody<GetTtyMessage> = params.parse()?;
    println!("params: {:#?}\n", message);
    let name = message.body.name;
    let (mut out_tty, mut in_tty) = {
        let tty_sessions = NAMED_TTY_SESSIONS.lock().unwrap();
        if !tty_sessions.contains_key(&name) {
            let mut error = RpcError::new(RpcErrorCode::ServerError(-404));
            error.message = "tty session not found".to_string();
            return Err(error);
        }
        let tty = tty_sessions.get(&name).unwrap();

        (tty.get_reader().unwrap(), tty.get_writer().unwrap())
    };

    out_tty.set_nonblocking(true).unwrap();
    out_tty.set_timeout(5000).unwrap();

    let id = Uuid::new_v4();
    let id = id.to_hyphenated().to_string();
    let tmp_dir = path_join!(env::temp_dir(), &id);
    fs::create_dir_all(&tmp_dir).unwrap();

    let input_path = path_join!(&tmp_dir, "in.sock");
    let output_path = path_join!(&tmp_dir, "out.sock");
    let io_paths = (input_path.clone(), output_path.clone());
    let for_thread = (name.to_owned(), io_paths);
    let out_listener = UnixListener::bind(&output_path).unwrap();
    let in_listener = UnixListener::bind(&input_path).unwrap();

    ThreadBuilder::new()
        .name("tty thread wrapper".to_string())
        .spawn(move || {

        let (name, io_paths) = for_thread;
        let (input_path, output_path) = io_paths;
        let (mut out_stream, _) = out_listener.accept().unwrap();
        let (mut in_stream, _) = in_listener.accept().unwrap();
        let mut in_stream_clone = in_stream.try_clone().unwrap();
        let mut out_stream_clone = out_stream.try_clone().unwrap();
        let out_thread = ThreadBuilder::new()
            .name("tty output thread".to_string())
            .spawn(move || {
                for bytes in out_tty.bytes() {
                    match bytes {
                        Ok(bytes) => {
                            let result = out_stream.write(&[bytes]);
                            if let Err(_) = result { break; }
                        },
                        Err(error) => {
                            if let io::ErrorKind::TimedOut = error.kind() {
                                let result = out_stream.write(&[]);
                                if let Err(_) = result { break; }
                                continue;
                            } else { break; }
                        }
                    }
                }
                out_stream.shutdown(Shutdown::Both);
                in_stream_clone.shutdown(Shutdown::Both);
            }).unwrap();
        let in_thread = ThreadBuilder::new()
            .name("tty input thread".to_string())
            .spawn(move || {
                let mut in_stream_clone = in_stream.try_clone().unwrap();
                for bytes in in_stream.bytes() {
                    match bytes {
                        Ok(bytes) => {
                            let result = in_tty.write(&[bytes]);
                            if let Err(_) = result { break; }
                        },
                        Err(_) => { break; }
                    }
                }
                in_stream_clone.shutdown(Shutdown::Both);
                out_stream_clone.shutdown(Shutdown::Both);
            }).unwrap();

        out_thread.join();
        println!("close out thread");
        in_thread.join();
        println!("close in thread");
        println!("close remote tty session");
        fs::remove_dir_all(&tmp_dir).unwrap();
    });

    Ok(json!({
        "input": input_path,
        "output": output_path
    }))
}

fn stop_container(params: RpcParams) -> Result<RpcValue, RpcError> {
    let message: MessageBody<StopContainerMessage> = params.parse()?;
    let name = message.body.name;
    {
        let mut named_invokers = NAMED_INVOKERS.lock().unwrap();
        let invoker = named_invokers.remove(&name.to_string());
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
    let message: MessageBody<RunContainerMessage> = params.clone().parse()?;
    let RunContainerMessage {
        name,
        rootfs, 
        workdir,
        rules,
        mounts,
        interface,
        entry,
        command,
        env: envs
    } = message.body;
    let mut invoker: AnyInvoker = Invoker::new();
    let command = format!("{} {}", entry, command);
    let mut jail_map = rules.as_jail_map().unwrap();
    jail_map.insert("path".try_into().unwrap(), rootfs.to_owned().try_into().unwrap());
    jail_map.insert("name".try_into().unwrap(), name.to_owned().try_into().unwrap());
    jail_map.insert("persist".try_into().unwrap(), true.try_into().unwrap());

    println!("{:#?}", jail_map);
    println!("mounts!");

    let devfs = path_join!(&rootfs, "/dev");
    let devfs = devfs.to_str().unwrap().to_owned();
    let fdescfs = path_join!(&rootfs, "/dev/fd");
    let fdescfs = fdescfs.to_str().unwrap().to_owned();
    let procfs = path_join!(&rootfs, "/proc");
    let procfs = procfs.to_str().unwrap().to_owned();
    let for_exec = (devfs, fdescfs, procfs);
    let for_unexec = for_exec.clone();

    exec_or_undo_all!(invoker, {
        exec: move {
            let (devfs, fdescfs, procfs) = for_exec.clone();
            mount_devfs(devfs, mount_options!({ "ruleset" => "4" })?, None)?;
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
        let src = rule_mount.get("src").unwrap().clone();
        let dst = rule_mount.get("dst").unwrap().clone();
        let dst = path_resolve!(&dst).unwrap();
        let dst = path_join!(&rootfs, &dst);
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

    let vnet = rules.get("vnet").unwrap_or(&"disabled".to_string())
        .clone();

    let exec_if = interface.to_string();
    let unexec_if = interface.to_string();
    if vnet == "new" && &interface != "" {
        exec_or_undo_all!(invoker, {
            exec: move {
                process::Command::new("ifconfig")
                    .args(&[&exec_if, "name", "eth0"])
                    .spawn()?
                    .wait()?;
                process::Command::new("ifconfig")
                    .args(&["eth0", "vnet", &jid.to_string()])
                    .spawn()?
                    .wait()?;

                Ok(Box::new(()) as Box<dyn Any>)
            },
            unexec: move {
                process::Command::new("ifconfig")
                    .args(&["eth0", "-vnet", &jid.to_string()])
                    .spawn()?
                    .wait()?;
                process::Command::new("ifconfig")
                    .args(&["eth0", "name", &unexec_if])
                    .spawn()?
                    .wait()?;

                Ok(())
            }
        }).unwrap();
    }

    println!("create_child[fork()]()");
    let fork_result = exec_or_undo_all!(invoker, {
        let result = forkpty()?;

        Ok(Box::new(result) as Box<Any>)
    }).unwrap();
    let fork_result: &ForkPtyResult = fork_result.downcast_ref::<ForkPtyResult>()
        .ok_or("fork_result cast error.")
        .unwrap()
        .to_owned();

    match &fork_result {
        ForkPtyResult::Parent(child, pty_master) => {
            let name = name.to_string();
            let for_exec = (name.clone(), Arc::new(pty_master.try_clone().unwrap()));
            let for_unexec = (name.clone(), );

            exec_or_undo_all!(invoker, {
                exec: move {
                    let (name, pty_master) = for_exec.clone();
                    let mut tty_sessions = NAMED_TTY_SESSIONS.lock()?;
                    tty_sessions.insert(name.to_string(), pty_master);

                    Ok(Box::new(()) as Box<Any>)
                },
                unexec: move {
                    let (name, ) = for_unexec.clone();
                    let mut tty_sessions = NAMED_TTY_SESSIONS.lock()?;
                    tty_sessions.remove(&name.to_string());

                    Ok(())
                }
            }).unwrap();
            {
                let mut named_invokers = NAMED_INVOKERS
                    .lock()
                    .unwrap();

                named_invokers.insert(name.to_string(), invoker);
            }
            let for_thread = (name.clone(), child.clone());

            thread::spawn(move || {
                let (name, child) = for_thread;
                let wait_result = child.wait(None).unwrap();
                let mut named_invokers = NAMED_INVOKERS.lock().unwrap();
                let invoker = named_invokers.remove(&name);
                if let Some(mut invoker) = invoker {
                    invoker.undo_all();
                }
                let mut subscribers = SUBSCRIBERS.lock().unwrap();

                for tx in subscribers.iter() {
                    let event = Events::ContainerStoped(name.to_string(), wait_result.into());
                    tx.send(event);
                }
            });
        },
        ForkPtyResult::Child(pid) => {
            libjail::attach(jid).unwrap();
            fs::create_dir_all(&workdir).unwrap();
            env::set_current_dir(&workdir).unwrap();
            for (key, value) in envs.iter() {
                env::set_var(key, value);
            }
            execvp(&CString::new("/bin/sh").unwrap(), &[
               CString::new("").unwrap(),
               CString::new("-c").unwrap(),
               CString::new(command).unwrap(),
            ]).unwrap();
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
