//! Helper methods to display transaction data in more human readable way.
use crate::{node::ShowCalls, resolver};

use colored::Colorize;
use serde::Deserialize;
use std::collections::HashMap;

use crate::fork::block_on;
use zksync_basic_types::H160;

use vm::vm::VmPartialExecutionResult;
use zksync_types::{vm_trace::Call, StorageLogQuery, StorageLogQueryType, VmEvent};

use lazy_static::lazy_static;

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub enum ContractType {
    System,
    Precompile,
    Popular,
    Unknown,
}

#[derive(Debug, Deserialize, Clone)]
pub struct KnownAddress {
    address: H160,
    name: String,
    contract_type: ContractType,
}

lazy_static! {
    /// Loads the known contact addresses from the JSON file.
    static ref KNOWN_ADDRESSES: HashMap<H160, KnownAddress> = {
        let json_value = serde_json::from_slice(include_bytes!("data/address_map.json")).unwrap();
        let pairs: Vec<KnownAddress> = serde_json::from_value(json_value).unwrap();

        pairs
            .into_iter()
            .map(|entry| (entry.address, entry))
            .collect()
    };
}

fn address_to_human_readable(address: H160) -> Option<String> {
    KNOWN_ADDRESSES
        .get(&address)
        .map(|known_address| match known_address.contract_type {
            ContractType::System => known_address.name.to_string(),
            ContractType::Precompile => format!("{}", known_address.name.dimmed()),
            ContractType::Popular => format!("{}", known_address.name.green()),
            ContractType::Unknown => known_address.name.to_string(),
        })
}

/// Pretty-prints event object
/// if skip_resolve is false, will try to contact openchain to resolve the topic hashes.
pub fn print_event(event: &VmEvent, resolve_hashes: bool) {
    let event = event.clone();
    block_on(async move {
        let mut tt: Vec<String> = vec![];
        if !resolve_hashes {
            tt = event.indexed_topics.iter().map(|t| t.to_string()).collect();
        } else {
            for topic in event.indexed_topics {
                let selector = resolver::decode_event_selector(&format!(
                    "0x{}",
                    hex::encode(topic.as_bytes())
                ))
                .await
                .unwrap();
                tt.push(selector.unwrap_or(format!("{:?}", topic)));
            }
        }

        log::info!(
            "{} {}",
            address_to_human_readable(event.address)
                .map(|x| format!("{:42}", x.blue()))
                .unwrap_or(format!("{:42}", format!("{:?}", event.address).blue())),
            tt.join(", ")
        );
    });
}

/// Pretty-prints contents of a 'call' - including subcalls.
/// If skip_resolve is false, will try to contact openchain to resolve the ABI names.
pub fn print_call(call: &Call, padding: usize, show_calls: &ShowCalls, resolve_hashes: bool) {
    let contract_type = KNOWN_ADDRESSES
        .get(&call.to)
        .cloned()
        .map(|known_address| known_address.contract_type)
        .unwrap_or(ContractType::Unknown);

    let should_print = match (&contract_type, &show_calls) {
        (_, ShowCalls::All) => true,
        (_, ShowCalls::None) => false,
        // now we're left only with 'user' and 'system'
        (ContractType::Unknown, _) => true,
        (ContractType::Popular, _) => true,
        (ContractType::Precompile, _) => false,
        // Now we're left with System
        (ContractType::System, ShowCalls::User) => false,
        (ContractType::System, ShowCalls::System) => true,
    };
    if should_print {
        let function_signature = if call.input.len() >= 4 {
            let sig = call.input.as_slice()[..4]
                .iter()
                .map(|byte| format!("{:02x}", byte))
                .collect::<Vec<_>>()
                .join("");

            if contract_type == ContractType::Precompile || !resolve_hashes {
                format!("{:>16}", sig)
            } else {
                block_on(async move {
                    let fetch = resolver::decode_function_selector(&sig).await.unwrap();
                    fetch.unwrap_or(format!("{:>16}", format!("0x{}", sig).dimmed()))
                })
            }
        } else {
            format!(
                "0x{}",
                call.input
                    .as_slice()
                    .iter()
                    .map(|byte| format!("{:02x}", byte))
                    .collect::<Vec<_>>()
                    .join("")
            )
        };

        let pretty_print = format!(
            "{}{:?} {} {} {} {} {}",
            " ".repeat(padding),
            call.r#type,
            address_to_human_readable(call.to)
                .map(|x| format!("{:<52}", x))
                .unwrap_or(format!("{:<52}", format!("{:?}", call.to).bold())),
            function_signature,
            call.revert_reason
                .as_ref()
                .map(|s| format!("Revert: {}", s))
                .unwrap_or_default(),
            call.error
                .as_ref()
                .map(|s| format!("Error: {}", s))
                .unwrap_or_default(),
            call.gas
        );

        if call.revert_reason.as_ref().is_some() || call.error.as_ref().is_some() {
            log::info!("{}", pretty_print.on_red());
        } else {
            log::info!("{}", pretty_print);
        }
    }
    for subcall in &call.calls {
        print_call(subcall, padding + 2, show_calls, resolve_hashes);
    }
}

pub fn print_logs(log_query: &StorageLogQuery) {
    let separator = "─".repeat(82);
    log::info!("{:<15} {:?}", "Type:", log_query.log_type);
    log::info!(
        "{:<15} {}",
        "Address:",
        address_to_human_readable(log_query.log_query.address)
            .unwrap_or(format!("{}", log_query.log_query.address))
    );
    log::info!("{:<15} {:#066x}", "Key:", log_query.log_query.key);

    log::info!(
        "{:<15} {:#066x}",
        "Read Value:",
        log_query.log_query.read_value
    );

    if log_query.log_type != StorageLogQueryType::Read {
        log::info!(
            "{:<15} {:#066x}",
            "Written Value:",
            log_query.log_query.written_value
        );
    }
    log::info!("{}", separator);
}

pub fn print_vm_details(result: &VmPartialExecutionResult) {
    log::info!("");
    log::info!("┌──────────────────────────┐");
    log::info!("│   VM EXECUTION RESULTS   │");
    log::info!("└──────────────────────────┘");

    log::info!("Cycles Used:          {}", result.cycles_used);
    log::info!("Computation Gas Used: {}", result.computational_gas_used);
    log::info!("Contracts Used:       {}", result.contracts_used);

    if let Some(revert_reason) = &result.revert_reason {
        log::info!("");
        log::info!(
            "{}",
            format!("[!] Revert Reason:    {}", revert_reason).on_red()
        );
    }

    log::info!("════════════════════════════");
}
