use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use anyhow::{Result, anyhow};
use solana_program::pubkey::Pubkey;
use std::str::FromStr;

/// 网络配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub network_type: String,
}

/// RPC配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    pub main_rpc_url: String,
    pub backup_rpc_url: String,
    pub strategy_rpc_url: String,
}

/// 身份验证配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub private_key: String,
}

/// MEV服务配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MevServiceConfig {
    pub enabled: bool,
    pub api_token: Option<String>,
    pub preferred_regions: Option<Vec<String>>,
}

/// MEV服务配置集合
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MevServicesConfig {
    pub jito: MevServiceConfig,
    pub nextblock: MevServiceConfig,
    pub bloxroute: MevServiceConfig,
    pub zeroslot: MevServiceConfig,
    pub temporal: MevServiceConfig,
}

/// gRPC配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcConfig {
    pub yellowstone_grpc_url: String,
    pub shredstream_url: String,
}

/// 交易配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingConfig {
    pub default_slippage_basis_points: u64,
    pub default_buy_amount_sol: u64,
    pub auto_handle_wsol: bool,
}

/// 优先费用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorityFeesConfig {
    pub compute_unit_limit: u32,
    pub compute_unit_price: u64,
    pub rpc_compute_unit_limit: u32,
    pub rpc_compute_unit_price: u64,
    pub buy_tip_fee: f64,
    pub sell_tip_fee: f64,
}

/// 策略配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    pub enable_take_profit_stop_loss: bool,
    pub enable_copy_trading: bool,
    pub max_concurrent_strategies: u32,
}

/// 日志配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub log_level: String,
    pub log_to_file: bool,
    pub log_file_path: String,
}

/// 监控配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    pub enable_price_monitoring: bool,
    pub price_cache_ttl_seconds: u64,
    pub enable_wallet_monitoring: bool,
    pub max_monitored_wallets: u32,
}

/// 止盈止损配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakeProfitStopLossConfig {
    pub enabled: bool,
    pub take_profit_percentage: f64,
    pub stop_loss_percentage: f64,
    pub buy_ratio: f64,
    pub max_buy_amount_sol: u64,
    pub min_buy_amount_sol: u64,
}

/// 钱包监控配置
#[derive(Debug, Clone, Deserialize)]
pub struct WalletMonitoringConfig {
    pub enabled: bool,
    pub wallet_addresses: Vec<String>,
    pub min_buy_amount_sol: u64,
    /// 是否启用DEX交易监听
    pub enable_dex_monitoring: bool,
    /// 要监听的DEX类型列表
    pub monitored_dex_types: Vec<String>,
    /// DEX交易监听间隔（秒）
    pub dex_monitoring_interval_seconds: u64,
    /// 价格解析缓存TTL（秒）
    pub price_cache_ttl_seconds: u64,
}

/// Raydium CPMM配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaydiumCpmmConfig {
    pub enabled: bool,
    pub pool_update_interval_seconds: u64,
    pub max_slippage_basis_points: u64,
}

/// 主配置结构体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub network: NetworkConfig,
    pub rpc: RpcConfig,
    pub authentication: AuthConfig,
    pub mev_services: MevServicesConfig,
    pub grpc: GrpcConfig,
    pub trading: TradingConfig,
    pub priority_fees: PriorityFeesConfig,
    pub strategy: StrategyConfig,
    pub logging: LoggingConfig,
    pub monitoring: MonitoringConfig,
    pub take_profit_stop_loss: TakeProfitStopLossConfig,
    pub wallet_monitoring: WalletMonitoringConfig,
    pub raydium_cpmm: RaydiumCpmmConfig,
}

impl AppConfig {
    /// 从文件加载配置
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let config_content = fs::read_to_string(path)
            .map_err(|e| anyhow!("读取配置文件失败: {}", e))?;
        
        let config: AppConfig = toml::from_str(&config_content)
            .map_err(|e| anyhow!("解析配置文件失败: {}", e))?;
        
        Ok(config)
    }

    /// 验证配置
    pub fn validate(&self) -> Result<()> {
        // 验证网络配置
        if !["mainnet-beta", "testnet", "devnet"].contains(&self.network.network_type.as_str()) {
            return Err(anyhow!("无效的网络类型: {}", self.network.network_type));
        }

        // 验证RPC URL
        if self.rpc.strategy_rpc_url.is_empty() {
            return Err(anyhow!("策略RPC URL不能为空"));
        }

        // 验证私钥
        if self.authentication.private_key.is_empty() {
            return Err(anyhow!("私钥不能为空"));
        }

        // 验证私钥格式
        if let Err(_) = bs58::decode(&self.authentication.private_key).into_vec() {
            return Err(anyhow!("私钥格式无效"));
        }

        // 验证交易配置
        if self.trading.default_slippage_basis_points == 0 {
            return Err(anyhow!("滑点不能为0"));
        }

        if self.trading.default_buy_amount_sol == 0 {
            return Err(anyhow!("默认买入金额不能为0"));
        }

        // 验证优先费用配置
        if self.priority_fees.compute_unit_limit == 0 {
            return Err(anyhow!("计算单元限制不能为0"));
        }

        // 验证策略配置
        if self.strategy.max_concurrent_strategies == 0 {
            return Err(anyhow!("最大并发策略数不能为0"));
        }

        // 验证日志配置
        if self.logging.log_to_file && self.logging.log_file_path.is_empty() {
            return Err(anyhow!("启用文件日志时必须指定日志文件路径"));
        }

        // 验证监控配置
        if self.monitoring.max_monitored_wallets == 0 {
            return Err(anyhow!("最大监控钱包数不能为0"));
        }

        // 验证止盈止损配置
        if self.take_profit_stop_loss.enabled {
            if self.take_profit_stop_loss.take_profit_percentage <= 0.0 {
                return Err(anyhow!("止盈百分比必须大于0"));
            }
            if self.take_profit_stop_loss.stop_loss_percentage <= 0.0 {
                return Err(anyhow!("止损百分比必须大于0"));
            }
            if self.take_profit_stop_loss.max_buy_amount_sol < self.take_profit_stop_loss.min_buy_amount_sol {
                return Err(anyhow!("最大买入金额不能小于最小买入金额"));
            }
        }

        // 验证钱包监控配置
        if self.wallet_monitoring.enabled {
            if self.wallet_monitoring.wallet_addresses.is_empty() {
                return Err(anyhow!("启用钱包监控时必须指定至少一个钱包地址"));
            }
            for address in &self.wallet_monitoring.wallet_addresses {
                if let Err(_) = Pubkey::from_str(address) {
                    return Err(anyhow!("无效的钱包地址: {}", address));
                }
            }
            
            // 验证DEX监听配置
            if self.wallet_monitoring.enable_dex_monitoring {
                if self.wallet_monitoring.monitored_dex_types.is_empty() {
                    return Err(anyhow!("启用DEX监听时必须指定至少一个DEX类型"));
                }
                if self.wallet_monitoring.dex_monitoring_interval_seconds == 0 {
                    return Err(anyhow!("DEX监听间隔不能为0"));
                }
                if self.wallet_monitoring.price_cache_ttl_seconds == 0 {
                    return Err(anyhow!("价格缓存TTL不能为0"));
                }
            }
        }

        // 验证Raydium CPMM配置
        if self.raydium_cpmm.enabled {
            if self.raydium_cpmm.pool_update_interval_seconds == 0 {
                return Err(anyhow!("池更新间隔不能为0"));
            }
            if self.raydium_cpmm.max_slippage_basis_points == 0 {
                return Err(anyhow!("最大滑点不能为0"));
            }
        }

        Ok(())
    }

    /// 获取日志级别
    pub fn get_log_level(&self) -> log::LevelFilter {
        match self.logging.log_level.to_lowercase().as_str() {
            "trace" => log::LevelFilter::Trace,
            "debug" => log::LevelFilter::Debug,
            "info" => log::LevelFilter::Info,
            "warn" => log::LevelFilter::Warn,
            "error" => log::LevelFilter::Error,
            _ => log::LevelFilter::Info,
        }
    }

    /// 获取配置摘要（用于日志，不包含敏感信息）
    pub fn get_summary(&self) -> String {
        format!(
            "网络: {}, 策略RPC: {}, 日志级别: {}, 止盈止损: {}, 钱包监控: {} (DEX监听: {}), Raydium CPMM: {}",
            self.network.network_type,
            if self.rpc.strategy_rpc_url.contains("mainnet") { "主网" } else { "测试网" },
            self.logging.log_level,
            if self.take_profit_stop_loss.enabled { "启用" } else { "禁用" },
            if self.wallet_monitoring.enabled { "启用" } else { "禁用" },
            if self.wallet_monitoring.enable_dex_monitoring { "启用" } else { "禁用" },
            if self.raydium_cpmm.enabled { "启用" } else { "禁用" }
        )
    }
}
