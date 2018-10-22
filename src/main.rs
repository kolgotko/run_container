extern crate libjail;
extern crate serde_json;
extern crate clap;

use libjail::*;
use std::error::Error;
use std::process::Command;


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

    let path = "/usr/local/jmaker/containers/ac-bt".to_string();
    let command = "top".to_string();

    println!("hello");

    let result = mount_nullfs("/usr/local/jmaker", "/mnt", vec![]);
    println!("{:?}", result);

    Ok(())

}
