extern crate serde_json;
extern crate libjail;

use libjail::*;
use libjail::Val as JailValue;
use std::net::{ Ipv4Addr, Ipv6Addr };
use std::error::Error;
use std::collections::HashMap;
use std::convert::TryInto;

type JailMap = HashMap<JailValue, JailValue>;

pub trait AsJailMap {
    fn as_jail_map(&self) -> Result<JailMap, Box<Error>>;
}

impl AsJailMap for HashMap<String, String> {
    fn as_jail_map(&self) -> Result<JailMap, Box<Error>> {
        let all_rules = &RULES_ALL;
        let mut result = HashMap::new();

        for (key, value) in self.iter() {
            let value = value.clone();
            let jail_key: JailValue = key.clone().try_into()?;
            let jail_value: JailValue = match all_rules.get(key) {
                Some(RuleType::Int) => {
                    let value_i32 = match value.as_str() {
                        "inherit" => JAIL_SYS_INHERIT,
                        "new" => JAIL_SYS_NEW,
                        "disable" => JAIL_SYS_DISABLE,
                        _ => value.parse::<i32>()?,
                    };
                    value_i32.try_into()?
                },
                Some(RuleType::String) => value.try_into()?,
                Some(RuleType::Ulong) => {
                    let value_u64: u64 = value.parse()?;
                    value_u64.try_into()?
                },
                Some(RuleType::Ip4) => {
                    value.parse::<Ipv4Addr>()?
                        .try_into()?
                },
                Some(RuleType::Ip6) => {
                    value.parse::<Ipv6Addr>()?
                        .try_into()?
                },
                _ => JailValue::Null,
            };
            result.insert(jail_key, jail_value); 
        }
        Ok(result)
    }
}
