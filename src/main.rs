use crate::cache::CacheConfig;
use crate::hardhat::{HardhatNamespaceImpl, HardhatNamespaceT};
use crate::node::{ShowGasDetails, ShowStorageLogs, ShowVMDetails};
use clap::{Parser, Subcommand, ValueEnum};
use configuration_api::ConfigurationApiNamespaceT;
use evm::{EvmNamespaceImpl, EvmNamespaceT};
use fork::{ForkDetails, ForkSource};
use logging_middleware::LoggingMiddleware;
use node::ShowCalls;
use simplelog::{
    ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, TermLogger, TerminalMode, WriteLogger,
};
use zks::ZkMockNamespaceImpl;

mod bootloader_debug;
mod cache;
mod configuration_api;
mod console_log;
mod deps;
mod evm;
mod fork;
mod formatter;
mod hardhat;
mod http_fork_source;
mod logging_middleware;
mod node;
mod resolver;
mod system_contracts;
mod testing;
mod utils;
mod zks;

use node::InMemoryNode;
use zksync_core::api_server::web3::namespaces::NetNamespace;

use std::{
    env,
    fs::File,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::FromStr,
};

use tracing::Level;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

use futures::{
    channel::oneshot,
    future::{self},
    FutureExt,
};
use jsonrpc_core::MetaIoHandler;
use zksync_basic_types::{L2ChainId, H160, H256};

use crate::{configuration_api::ConfigurationApiNamespace, node::TEST_NODE_NETWORK_ID};
use zksync_core::api_server::web3::backend_jsonrpc::namespaces::{
    eth::EthNamespaceT, net::NetNamespaceT, zks::ZksNamespaceT,
};

/// List of wallets (address, private key) that we seed with tokens at start.
pub const RICH_WALLETS: [(&str, &str); 10] = [
    (
        "0x36615Cf349d7F6344891B1e7CA7C72883F5dc049",
        "0x7726827caac94a7f9e1b160f7ea819f172f7b6f9d2a97f992c38edeab82d4110",
    ),
    (
        "0xa61464658AfeAf65CccaaFD3a512b69A83B77618",
        "0xac1e735be8536c6534bb4f17f06f6afc73b2b5ba84ac2cfb12f7461b20c0bbe3",
    ),
    (
        "0x0D43eB5B8a47bA8900d84AA36656c92024e9772e",
        "0xd293c684d884d56f8d6abd64fc76757d3664904e309a0645baf8522ab6366d9e",
    ),
    (
        "0xA13c10C0D5bd6f79041B9835c63f91de35A15883",
        "0x850683b40d4a740aa6e745f889a6fdc8327be76e122f5aba645a5b02d0248db8",
    ),
    (
        "0x8002cD98Cfb563492A6fB3E7C8243b7B9Ad4cc92",
        "0xf12e28c0eb1ef4ff90478f6805b68d63737b7f33abfa091601140805da450d93",
    ),
    (
        "0x4F9133D1d3F50011A6859807C837bdCB31Aaab13",
        "0xe667e57a9b8aaa6709e51ff7d093f1c5b73b63f9987e4ab4aa9a5c699e024ee8",
    ),
    (
        "0xbd29A1B981925B94eEc5c4F1125AF02a2Ec4d1cA",
        "0x28a574ab2de8a00364d5dd4b07c4f2f574ef7fcc2a86a197f65abaec836d1959",
    ),
    (
        "0xedB6F5B4aab3dD95C7806Af42881FF12BE7e9daa",
        "0x74d8b3a188f7260f67698eb44da07397a298df5427df681ef68c45b34b61f998",
    ),
    (
        "0xe706e60ab5Dc512C36A4646D719b889F398cbBcB",
        "0xbe79721778b48bcc679b78edac0ce48306a8578186ffcb9f2ee455ae6efeace1",
    ),
    (
        "0xE90E12261CCb0F3F7976Ae611A29e84a6A85f424",
        "0x3eb15da85647edd9a1159a4a13b9e7c56877c4eb33f614546d4db06a51868b1c",
    ),
];

#[allow(clippy::too_many_arguments)]
async fn build_json_http<
    S: std::marker::Sync + std::marker::Send + 'static + ForkSource + std::fmt::Debug,
>(
    addr: SocketAddr,
    log_level_filter: LevelFilter,
    node: InMemoryNode<S>,
    net: NetNamespace,
    config_api: ConfigurationApiNamespace<S>,
    evm: EvmNamespaceImpl<S>,
    zks: ZkMockNamespaceImpl<S>,
    hardhat: HardhatNamespaceImpl<S>,
) -> tokio::task::JoinHandle<()> {
    let (sender, recv) = oneshot::channel::<()>();

    let io_handler = {
        let mut io = MetaIoHandler::with_middleware(LoggingMiddleware::new(log_level_filter));
        io.extend_with(node.to_delegate());
        io.extend_with(net.to_delegate());
        io.extend_with(config_api.to_delegate());
        io.extend_with(evm.to_delegate());
        io.extend_with(zks.to_delegate());
        io.extend_with(hardhat.to_delegate());
        io
    };

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .unwrap();

        let server = jsonrpc_http_server::ServerBuilder::new(io_handler)
            .threads(1)
            .event_loop_executor(runtime.handle().clone())
            .start_http(&addr)
            .unwrap();

        server.wait();
        let _ = sender;
    });

    tokio::spawn(recv.map(drop))
}

/// Log filter level for the node.
#[derive(Debug, Clone, ValueEnum)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<LogLevel> for LevelFilter {
    fn from(value: LogLevel) -> Self {
        match value {
            LogLevel::Trace => LevelFilter::Trace,
            LogLevel::Debug => LevelFilter::Debug,
            LogLevel::Info => LevelFilter::Info,
            LogLevel::Warn => LevelFilter::Warn,
            LogLevel::Error => LevelFilter::Error,
        }
    }
}

/// Cache type config for the node.
#[derive(ValueEnum, Debug, Clone)]
enum CacheType {
    None,
    Memory,
    Disk,
}

#[derive(Debug, Parser)]
#[command(author = "Matter Labs", version, about = "Test Node", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[arg(long, default_value = "8011")]
    /// Port to listen on - default: 8011
    port: u16,
    #[arg(long, default_value = "none")]
    /// Show call debug information
    show_calls: ShowCalls,
    #[arg(long, default_value = "none")]
    /// Show storage log information
    show_storage_logs: ShowStorageLogs,
    #[arg(long, default_value = "none")]
    /// Show VM details information
    show_vm_details: ShowVMDetails,

    #[arg(long, default_value = "none")]
    /// Show Gas details information
    show_gas_details: ShowGasDetails,

    #[arg(long)]
    /// If true, the tool will try to contact openchain to resolve the ABI & topic names.
    /// It will make debug log more readable, but will decrease the performance.
    resolve_hashes: bool,

    #[arg(long)]
    /// If true, will load the locally compiled system contracts (useful when doing changes to system contracts or bootloader)
    dev_use_local_contracts: bool,

    /// Log filter level - default: info
    #[arg(long, default_value = "info")]
    log: LogLevel,

    /// Log file path - default: era_test_node.log
    #[arg(long, default_value = "era_test_node.log")]
    log_file_path: String,

    /// Cache type, can be one of `none`, `memory`, or `disk` - default: "disk"
    #[arg(long, default_value = "disk")]
    cache: CacheType,

    /// If true, will reset the local `disk` cache.
    #[arg(long)]
    reset_cache: bool,

    /// Cache directory location for `disk` cache - default: ".cache"
    #[arg(long, default_value = ".cache")]
    cache_dir: String,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Starts a new empty local network.
    #[command(name = "run")]
    Run,
    /// Starts a local network that is a fork of another network.
    #[command(name = "fork")]
    Fork(ForkArgs),
    /// Starts a local network that is a fork of another network, and replays a given TX on it.
    #[command(name = "replay_tx")]
    ReplayTx(ReplayArgs),
}

#[derive(Debug, Parser)]
struct ForkArgs {
    /// Whether to fork from existing network.
    /// If not set - will start a new network from genesis.
    /// If set - will try to fork a remote network. Possible values:
    ///  - mainnet
    ///  - testnet
    ///  - http://XXX:YY
    network: String,
    #[arg(long)]
    // Fork at a given L2 miniblock height.
    // If not set - will use the current finalized block from the network.
    fork_at: Option<u64>,
}
#[derive(Debug, Parser)]
struct ReplayArgs {
    /// Whether to fork from existing network.
    /// If not set - will start a new network from genesis.
    /// If set - will try to fork a remote network. Possible values:
    ///  - mainnet
    ///  - testnet
    ///  - http://XXX:YY
    network: String,
    /// Transaction hash to replay.
    tx: H256,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opt = Cli::parse();

    let log_level_filter = LevelFilter::from(opt.log);
    let log_config = ConfigBuilder::new()
        .add_filter_allow_str("era_test_node")
        .build();
    CombinedLogger::init(vec![
        TermLogger::new(
            log_level_filter,
            log_config.clone(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(
            log_level_filter,
            log_config,
            File::create(opt.log_file_path).unwrap(),
        ),
    ])
    .expect("failed instantiating logger");

    if opt.dev_use_local_contracts {
        if let Some(path) = env::var_os("ZKSYNC_HOME") {
            log::info!("+++++ Reading local contracts from {:?} +++++", path);
        }
    }
    let cache_config = match opt.cache {
        CacheType::None => CacheConfig::None,
        CacheType::Memory => CacheConfig::Memory,
        CacheType::Disk => CacheConfig::Disk {
            dir: opt.cache_dir,
            reset: opt.reset_cache,
        },
    };

    let filter = EnvFilter::from_default_env();
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .with_env_filter(filter)
        .finish();

    // Initialize the subscriber
    tracing::subscriber::set_global_default(subscriber).expect("failed to set tracing subscriber");

    let fork_details = match &opt.command {
        Command::Run => None,
        Command::Fork(fork) => {
            Some(ForkDetails::from_network(&fork.network, fork.fork_at, cache_config).await)
        }
        Command::ReplayTx(replay_tx) => {
            Some(ForkDetails::from_network_tx(&replay_tx.network, replay_tx.tx, cache_config).await)
        }
    };

    // If we're replaying the transaction, we need to sync to the previous block
    // and then replay all the transactions that happened in
    let transactions_to_replay = if let Command::ReplayTx(replay_tx) = &opt.command {
        fork_details
            .as_ref()
            .unwrap()
            .get_earlier_transactions_in_same_block(replay_tx.tx)
            .await
    } else {
        vec![]
    };
    let system_contracts_options = if opt.dev_use_local_contracts {
        system_contracts::Options::Local
    } else {
        system_contracts::Options::BuiltIn
    };

    let node = InMemoryNode::new(
        fork_details,
        opt.show_calls,
        opt.show_storage_logs,
        opt.show_vm_details,
        opt.show_gas_details,
        opt.resolve_hashes,
        &system_contracts_options,
    );

    if !transactions_to_replay.is_empty() {
        let _ = node.apply_txs(transactions_to_replay);
    }

    log::info!("Rich Accounts");
    log::info!("=============");
    for (index, wallet) in RICH_WALLETS.iter().enumerate() {
        let address = wallet.0;
        let private_key = wallet.1;
        node.set_rich_account(H160::from_str(address).unwrap());
        log::info!("Account #{}: {} (1_000_000_000_000 ETH)", index, address);
        log::info!("Private Key: {}", private_key);
        log::info!("");
    }

    let net = NetNamespace::new(L2ChainId(TEST_NODE_NETWORK_ID));
    let config_api = ConfigurationApiNamespace::new(node.get_inner());
    let evm = EvmNamespaceImpl::new(node.get_inner());
    let zks = ZkMockNamespaceImpl::new(node.get_inner());
    let hardhat = HardhatNamespaceImpl::new(node.get_inner());

    let threads = build_json_http(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), opt.port),
        log_level_filter,
        node,
        net,
        config_api,
        evm,
        zks,
        hardhat,
    )
    .await;

    log::info!("========================================");
    log::info!("  Node is ready at 127.0.0.1:{}", opt.port);
    log::info!("========================================");

    future::select_all(vec![threads]).await.0.unwrap();

    Ok(())
}
