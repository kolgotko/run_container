extern crate serde_json;
extern crate libjail;

use libjail::*;
use libjail::Val as JailValue;
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::error::Error;
use std::collections::HashMap;
use std::convert::TryInto;

type JailMap = HashMap<Val, Val>;

pub trait AsJailMap {
    fn as_jail_map(&self) -> Result<JailMap, Box<Error>>;
}

impl AsJailMap for JsonMap<String, JsonValue> {

    fn as_jail_map(&self) -> Result<JailMap, Box<Error>> {

        let all_rules = &RULES_ALL;
        let mut out_map: JailMap = HashMap::new();

        for (rule, rule_type) in all_rules.iter() {

            let jail_key: JailValue = rule.clone().try_into()?;

            let value = self.get(rule);
            if value.is_none() { continue; }
            let value = value.unwrap();

            match rule_type {
                RuleType::Int => {

                    let int = match value.clone() {
                        JsonValue::Bool(value) => value as i32,
                        JsonValue::String(value) => {
                            if value == "inherit" { JAIL_SYS_INHERIT }
                            else if value == "new" { JAIL_SYS_NEW }
                            else if value == "disable" { JAIL_SYS_DISABLE }
                            else {
                                value.parse::<i32>()?
                            }
                        }
                        _ => value.as_u64().ok_or("type error")? as i32,
                    };
                    out_map.insert(jail_key, int.try_into()?);
                },
                RuleType::Ulong => {
                    let int = value.as_u64().ok_or("type error")?;
                    out_map.insert(jail_key, int.try_into()?);
                },
                RuleType::String => {
                    let st = value.as_str().ok_or("type error")?;;
                    out_map.insert(jail_key, st.try_into()?);
                },
                RuleType::Ip4 => {

                    let ip = match value {
                        _ => "127.0.0.1".parse::<Ipv4Addr>()?,
                        JsonValue::String(ip_str) => {
                            ip_str.parse::<Ipv4Addr>()?
                        },
                    };

                    out_map.insert(jail_key, ip.try_into()?);
                },
                RuleType::Ip6 => {

                    let ip = match value {
                        _ => "::1".parse::<Ipv6Addr>()?,
                        JsonValue::String(ip_str) => {
                            ip_str.parse::<Ipv6Addr>()?
                        },
                    };

                    out_map.insert(jail_key, ip.try_into()?);
                },
                _ => { Err("unknown type")? },
            }

        }

        Ok(out_map)

    }

}

