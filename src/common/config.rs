use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use anyhow::{Result, Context};

/// 配置结构体，对应 config.toml 文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub network: NetworkConfig,
    pub rpc: RpcConfig,
    pub authentication: AuthenticationConfig,
    pub mev_services: MevServicesConfig,
    pub grpc: GrpcConfig,
    pub trading: TradingConfig,
    pub priority_fees: PriorityFeesConfig,
    pub strategy: StrategyConfig,
    pub logging: LoggingConfig,
    pub monitoring: MonitoringConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub network_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    pub main_rpc_url: String,
    pub backup_rpc_url: String,
    pub strategy_rpc_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticationConfig {
    pub private_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MevServicesConfig {
    pub jito: JitoConfig,
    pub nextblock: NextBlockConfig,
    pub bloxroute: BloxrouteConfig,
    pub zeroslot: ZeroSlotConfig,
    pub temporal: TemporalConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JitoConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextBlockConfig {
    pub enabled: bool,
    pub api_token: String,
    pub preferred_regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloxrouteConfig {
    pub enabled: bool,
    pub api_token: String,
    pub preferred_regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZeroSlotConfig {
    pub enabled: bool,
    pub api_token: String,
    pub preferred_regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalConfig {
    pub enabled: bool,
    pub api_token: String,
    pub preferred_regions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcConfig {
    pub yellowstone_grpc_url: String,
    pub shredstream_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingConfig {
    pub default_slippage_basis_points: u16,
    pub default_buy_amount_sol: u64,
    pub auto_handle_wsol: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorityFeesConfig {
    pub compute_unit_limit: u32,
    pub compute_unit_price: u64,
    pub rpc_compute_unit_limit: u32,
    pub rpc_compute_unit_price: u64,
    pub buy_tip_fee: f64,
    pub sell_tip_fee: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    pub enable_take_profit_stop_loss: bool,
    pub enable_copy_trading: bool,
    pub max_concurrent_strategies: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub log_level: String,
    pub log_to_file: bool,
    pub log_file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    pub enable_price_monitoring: bool,
    pub price_cache_ttl_seconds: u64,
    pub enable_wallet_monitoring: bool,
    pub max_monitored_wallets: usize,
}

impl Config {
    /// 从配置文件加载配置
    pub fn load() -> Result<Self> {
        let config_path = "config.toml";
        
        if !Path::new(config_path).exists() {
            return Err(anyhow::anyhow!(
                "配置文件 config.toml 不存在。请复制 config.example.toml 为 config.toml 并填入您的配置信息。"
            ));
        }

        let config_content = fs::read_to_string(config_path)
            .context("无法读取配置文件")?;

        let config: Config = toml::from_str(&config_content)
            .context("配置文件格式错误，请检查 TOML 语法")?;

        // 验证配置
        config.validate()?;

        Ok(config)
    }

    /// 验证配置的有效性
    fn validate(&self) -> Result<()> {
        // 检查私钥格式
        if self.authentication.private_key == "YOUR_PRIVATE_KEY_BASE58" {
            return Err(anyhow::anyhow!("请配置您的实际私钥"));
        }

        // 检查 API key 是否已配置
        if self.mev_services.nextblock.enabled && 
           self.mev_services.nextblock.api_token == "YOUR_NEXTBLOCK_API_TOKEN" {
            return Err(anyhow::anyhow!("请配置 NextBlock API token"));
        }

        if self.mev_services.bloxroute.enabled && 
           self.mev_services.bloxroute.api_token == "YOUR_BLOXROUTE_API_TOKEN" {
            return Err(anyhow::anyhow!("请配置 Bloxroute API token"));
        }

        if self.mev_services.zeroslot.enabled && 
           self.mev_services.zeroslot.api_token == "YOUR_ZEROSLOT_API_TOKEN" {
            return Err(anyhow::anyhow!("请配置 ZeroSlot API token"));
        }

        if self.mev_services.temporal.enabled && 
           self.mev_services.temporal.api_token == "YOUR_TEMPORAL_API_TOKEN" {
            return Err(anyhow::anyhow!("请配置 Temporal API token"));
        }

        // 检查 RPC URL 是否已配置
        if self.rpc.main_rpc_url.contains("YOUR_HELIUS_API_KEY") {
            return Err(anyhow::anyhow!("请配置 Helius RPC API key"));
        }

        if self.rpc.strategy_rpc_url.contains("YOUR_STRATEGY_API_KEY") {
            return Err(anyhow::anyhow!("请配置策略 RPC API key"));
        }

        Ok(())
    }

    /// 获取启用的 MEV 服务配置
    pub fn get_enabled_mev_services(&self) -> Vec<(&str, &str, Vec<&str>)> {
        let mut services = Vec::new();

        if self.mev_services.jito.enabled {
            services.push(("jito", "", vec![]));
        }

        if self.mev_services.nextblock.enabled {
            services.push(("nextblock", &self.mev_services.nextblock.api_token, 
                         self.mev_services.nextblock.preferred_regions.iter().map(|s| s.as_str()).collect()));
        }

        if self.mev_services.bloxroute.enabled {
            services.push(("bloxroute", &self.mev_services.bloxroute.api_token,
                         self.mev_services.bloxroute.preferred_regions.iter().map(|s| s.as_str()).collect()));
        }

        if self.mev_services.zeroslot.enabled {
            services.push(("zeroslot", &self.mev_services.zeroslot.api_token,
                         self.mev_services.zeroslot.preferred_regions.iter().map(|s| s.as_str()).collect()));
        }

        if self.mev_services.temporal.enabled {
            services.push(("temporal", &self.mev_services.temporal.api_token,
                         self.mev_services.temporal.preferred_regions.iter().map(|s| s.as_str()).collect()));
        }

        services
    }

    /// 获取主要 RPC URL
    pub fn get_main_rpc_url(&self) -> &str {
        &self.rpc.main_rpc_url
    }

    /// 获取备用 RPC URL
    pub fn get_backup_rpc_url(&self) -> &str {
        &self.rpc.backup_rpc_url
    }

    /// 获取策略 RPC URL
    pub fn get_strategy_rpc_url(&self) -> &str {
        &self.rpc.strategy_rpc_url
    }

    /// 获取私钥
    pub fn get_private_key(&self) -> &str {
        &self.authentication.private_key
    }

    /// 获取优先费用配置
    pub fn get_priority_fees(&self) -> &PriorityFeesConfig {
        &self.priority_fees
    }

    /// 获取交易配置
    pub fn get_trading_config(&self) -> &TradingConfig {
        &self.trading
    }
}

/// 配置管理器
pub struct ConfigManager {
    config: Config,
}

impl ConfigManager {
    /// 创建新的配置管理器
    pub fn new() -> Result<Self> {
        let config = Config::load()?;
        Ok(Self { config })
    }

    /// 获取配置引用
    pub fn get_config(&self) -> &Config {
        &self.config
    }

    /// 重新加载配置
    pub fn reload(&mut self) -> Result<()> {
        self.config = Config::load()?;
        Ok(())
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new().expect("无法加载默认配置")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        // 测试默认配置验证失败
        let config = Config {
            network: NetworkConfig { network_type: "mainnet".to_string() },
            rpc: RpcConfig {
                main_rpc_url: "https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_API_KEY".to_string(),
                backup_rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
                strategy_rpc_url: "https://example.com?api-key=YOUR_STRATEGY_API_KEY".to_string(),
            },
            authentication: AuthenticationConfig {
                private_key: "YOUR_PRIVATE_KEY_BASE58".to_string(),
            },
            mev_services: MevServicesConfig {
                jito: JitoConfig { enabled: true },
                nextblock: NextBlockConfig {
                    enabled: true,
                    api_token: "YOUR_NEXTBLOCK_API_TOKEN".to_string(),
                    preferred_regions: vec!["Frankfurt".to_string()],
                },
                bloxroute: BloxrouteConfig {
                    enabled: true,
                    api_token: "YOUR_BLOXROUTE_API_TOKEN".to_string(),
                    preferred_regions: vec!["Frankfurt".to_string()],
                },
                zeroslot: ZeroSlotConfig {
                    enabled: true,
                    api_token: "YOUR_ZEROSLOT_API_TOKEN".to_string(),
                    preferred_regions: vec!["Frankfurt".to_string()],
                },
                temporal: TemporalConfig {
                    enabled: true,
                    api_token: "YOUR_TEMPORAL_API_TOKEN".to_string(),
                    preferred_regions: vec!["Frankfurt".to_string()],
                },
            },
            grpc: GrpcConfig {
                yellowstone_grpc_url: "https://example.com".to_string(),
                shredstream_url: "http://127.0.0.1:10800".to_string(),
            },
            trading: TradingConfig {
                default_slippage_basis_points: 100,
                default_buy_amount_sol: 100_000,
                auto_handle_wsol: true,
            },
            priority_fees: PriorityFeesConfig {
                compute_unit_limit: 190000,
                compute_unit_price: 1000000,
                rpc_compute_unit_limit: 500000,
                rpc_compute_unit_price: 500000,
                buy_tip_fee: 0.001,
                sell_tip_fee: 0.0001,
            },
            strategy: StrategyConfig {
                enable_take_profit_stop_loss: true,
                enable_copy_trading: true,
                max_concurrent_strategies: 10,
            },
            logging: LoggingConfig {
                log_level: "info".to_string(),
                log_to_file: true,
                log_file_path: "logs/sol_trade_sdk.log".to_string(),
            },
            monitoring: MonitoringConfig {
                enable_price_monitoring: true,
                price_cache_ttl_seconds: 300,
                enable_wallet_monitoring: true,
                max_monitored_wallets: 100,
            },
        };

        // 验证应该失败
        assert!(config.validate().is_err());
    }
}
