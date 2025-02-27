use vm::vm_with_bootloader::TxExecutionMode;
use zksync_contracts::{
    read_playground_block_bootloader_bytecode, read_proved_block_bootloader_bytecode,
    read_sys_contract_bytecode, read_zbin_bytecode, BaseSystemContracts, ContractLanguage,
    SystemContractCode,
};
use zksync_types::system_contracts::get_system_smart_contracts;
use zksync_utils::{bytecode::hash_bytecode, bytes_to_be_words};

use crate::deps::system_contracts::{bytecode_from_slice, COMPILED_IN_SYSTEM_CONTRACTS};

pub enum Options {
    // Use the compiled-in contracts
    BuiltIn,
    // Load the contracts bytecode at runtime from ZKSYNC_HOME
    Local,
    // Don't verify the signatures (used only for testing - for example Forge).
    BuiltInWithoutSecurity,
}

/// Holds the system contracts (and bootloader) that are used by the in-memory node.
pub struct SystemContracts {
    pub baseline_contracts: BaseSystemContracts,
    pub playground_contracts: BaseSystemContracts,
    pub fee_estimate_contracts: BaseSystemContracts,
}

pub fn get_deployed_contracts(options: &Options) -> Vec<zksync_types::block::DeployedContract> {
    match options {
        Options::BuiltIn | Options::BuiltInWithoutSecurity => COMPILED_IN_SYSTEM_CONTRACTS.clone(),
        Options::Local => get_system_smart_contracts(),
    }
}

impl Default for SystemContracts {
    /// Creates SystemContracts that use compiled-in contracts.
    fn default() -> Self {
        SystemContracts::from_options(&Options::BuiltIn)
    }
}

impl SystemContracts {
    /// Creates the SystemContracts that use the complied contracts from ZKSYNC_HOME path.
    /// These are loaded at binary runtime.
    pub fn from_options(options: &Options) -> Self {
        Self {
            baseline_contracts: baseline_contracts(options),
            playground_contracts: playground(options),
            fee_estimate_contracts: fee_estimate_contracts(options),
        }
    }
    pub fn contacts_for_l2_call(&self) -> &BaseSystemContracts {
        self.contracts(TxExecutionMode::EthCall {
            missed_storage_invocation_limit: 1,
        })
    }

    pub fn contracts_for_fee_estimate(&self) -> &BaseSystemContracts {
        self.contracts(TxExecutionMode::EstimateFee {
            missed_storage_invocation_limit: 1,
        })
    }

    pub fn contracts(&self, execution_mode: TxExecutionMode) -> &BaseSystemContracts {
        match execution_mode {
            // 'real' contracts, that do all the checks.
            TxExecutionMode::VerifyExecute => &self.baseline_contracts,
            // Ignore invalid sigatures. These requests are often coming unsigned, and they keep changing the
            // gas limit - so the signatures are often not matching.
            TxExecutionMode::EstimateFee { .. } => &self.fee_estimate_contracts,
            // Read-only call - don't check signatures, have a lower (fixed) gas limit.
            TxExecutionMode::EthCall { .. } => &self.playground_contracts,
        }
    }
}

/// Creates BaseSystemContracts object with a specific bootloader.
fn bsc_load_with_bootloader(
    bootloader_bytecode: Vec<u8>,
    options: &Options,
) -> BaseSystemContracts {
    let hash = hash_bytecode(&bootloader_bytecode);

    let bootloader = SystemContractCode {
        code: bytes_to_be_words(bootloader_bytecode),
        hash,
    };

    let bytecode = match options {
        Options::BuiltIn => bytecode_from_slice(
            "DefaultAccount",
            include_bytes!("deps/contracts/DefaultAccount.json"),
        ),
        Options::Local => read_sys_contract_bytecode("", "DefaultAccount", ContractLanguage::Sol),
        Options::BuiltInWithoutSecurity => bytecode_from_slice(
            "DefaultAccountNoSecurity",
            include_bytes!("deps/contracts/DefaultAccountNoSecurity.json"),
        ),
    };

    let hash = hash_bytecode(&bytecode);

    let default_aa = SystemContractCode {
        code: bytes_to_be_words(bytecode),
        hash,
    };

    BaseSystemContracts {
        bootloader,
        default_aa,
    }
}

/// BaseSystemContracts with playground bootloader -  used for handling 'eth_calls'.
pub fn playground(options: &Options) -> BaseSystemContracts {
    let bootloader_bytecode = match options {
        Options::BuiltIn | Options::BuiltInWithoutSecurity => {
            include_bytes!("deps/contracts/playground_block.yul.zbin").to_vec()
        }
        Options::Local => read_playground_block_bootloader_bytecode(),
    };

    bsc_load_with_bootloader(bootloader_bytecode, options)
}

/// Returns the system contracts for fee estimation.
///
/// # Arguments
///
/// * `use_local_contracts` - A boolean indicating whether to use local contracts or not.
///
/// # Returns
///
/// A `BaseSystemContracts` struct containing the system contracts used for handling 'eth_estimateGas'.
/// It sets ENSURE_RETURNED_MAGIC to 0 and BOOTLOADER_TYPE to 'playground_block'
pub fn fee_estimate_contracts(options: &Options) -> BaseSystemContracts {
    let bootloader_bytecode = match options {
        Options::BuiltIn |
        Options::BuiltInWithoutSecurity => {
            include_bytes!("deps/contracts/fee_estimate.yul.zbin").to_vec()
        }
        Options::Local =>
            read_zbin_bytecode("etc/system-contracts/bootloader/build/artifacts/fee_estimate.yul/fee_estimate.yul.zbin")
    };

    bsc_load_with_bootloader(bootloader_bytecode, options)
}

pub fn baseline_contracts(options: &Options) -> BaseSystemContracts {
    let bootloader_bytecode = match options {
        Options::BuiltIn | Options::BuiltInWithoutSecurity => {
            include_bytes!("deps/contracts/proved_block.yul.zbin").to_vec()
        }
        Options::Local => read_proved_block_bootloader_bytecode(),
    };
    bsc_load_with_bootloader(bootloader_bytecode, options)
}
