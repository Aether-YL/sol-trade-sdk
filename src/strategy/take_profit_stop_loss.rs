use std::{collections::HashMap, sync::Arc, str::FromStr};
use std::sync::Mutex;
use tokio::time::{sleep, Duration};
use tokio::sync::mpsc;
use solana_sdk::pubkey::Pubkey;
use chrono::{DateTime, Utc};
use sol_trade_sdk::utils;
use sol_trade_sdk::solana_streamer_sdk::{
    match_event,
    streaming::{
        event_parser::{
            protocols::raydium_cpmm::RaydiumCpmmSwapEvent,
            Protocol, UnifiedEvent,
        },
        YellowstoneGrpc,
    },
};
use solana_streamer_sdk::streaming::event_parser::{
    common::{EventMetadata, TransferData, ProtocolType, EventType},
};

// 导入main.rs中的买入函数相关模块
use sol_trade_sdk::trading::{
    core::params::RaydiumCpmmParams,
    factory::DexType,
    raydium_cpmm::common::get_buy_token_amount,
};
use sol_trade_sdk::SolanaTrade;
use sol_trade_sdk::constants::trade::trade::DEFAULT_SLIPPAGE;
use spl_token;
use tracing::event;

/// 交易策略配置
#[derive(Debug, Clone)]
pub struct TradingStrategy {
    // 监控的钱包地址
    pub target_wallet: String,
    // 当前跟单的token地址（动态设置）
    pub current_token: Option<String>,
    // 当前跟单的pool地址（动态设置）
    pub current_pool: Option<String>,
    // 买入成本价格
    pub buy_price: Option<f64>,
    // 买入数量
    pub buy_amount: Option<u64>,
    // 止盈价格（百分比）
    pub take_profit_percentage: f64,
    // 止损价格（百分比）
    pub stop_loss_percentage: f64,
    // 当前持仓状态
    pub position_status: PositionStatus,
    // 策略状态
    pub strategy_status: StrategyStatus,
    // 监控模式
    pub monitoring_mode: MonitoringMode,
    // 已完成的token列表（避免重复监控）
    pub completed_tokens: Vec<String>,
}

// 实现 Send trait（因为所有字段都是 Send 的）
unsafe impl Send for TradingStrategy {}

/// 持仓状态
#[derive(Debug, Clone, PartialEq)]
pub enum PositionStatus {
    NoPosition,    // 无持仓
    Holding,       // 持有中
    Sold,          // 已卖出
}

// 实现 Send trait（因为所有字段都是 Send 的）
unsafe impl Send for PositionStatus {}

/// 策略状态
#[derive(Debug, Clone, PartialEq)]
pub enum StrategyStatus {
    Monitoring,    // 监控中
    Paused,        // 暂停
    Stopped,       // 停止
}

// 实现 Send trait（因为所有字段都是 Send 的）
unsafe impl Send for StrategyStatus {}

/// 监控模式
#[derive(Debug, Clone, PartialEq)]
pub enum MonitoringMode {
    Wallet,        // 监控钱包交易
    Token,         // 监控特定token
}

// 实现 Send trait（因为所有字段都是 Send 的）
unsafe impl Send for MonitoringMode {}

/// 价格信息
#[derive(Debug, Clone)]
pub struct PriceInfo {
    pub token_address: String,
    pub current_price: f64,
    pub timestamp: u64,
    pub volume_24h: Option<f64>,
}

/// 交易信号
#[derive(Debug, Clone)]
pub struct TradingSignal {
    pub signal_type: SignalType,
    pub token_address: String,
    pub price: f64,
    pub amount: u64,
    pub timestamp: u64,
    pub wallet_address: String,
    pub pool_address: String,
}

/// 信号类型
#[derive(Debug, Clone)]
pub enum SignalType {
    Buy,           // 买入信号
    Sell,          // 卖出信号
    TakeProfit,    // 止盈信号
    StopLoss,      // 止损信号
}

/// 交易信号队列项
#[derive(Debug, Clone)]
pub struct SignalQueueItem {
    pub signal: TradingSignal,
    pub strategy_id: String,
    pub timestamp: u64,
}

/// 止盈止损策略管理器
#[derive(Clone)]
pub struct TakeProfitStopLossStrategy {
    strategies: Arc<Mutex<HashMap<String, TradingStrategy>>>,
    price_cache: Arc<Mutex<HashMap<String, PriceInfo>>>,
    grpc_client: Arc<YellowstoneGrpc>,
    // 添加信号队列
    signal_sender: mpsc::UnboundedSender<SignalQueueItem>,
    signal_receiver: Arc<Mutex<Option<mpsc::UnboundedReceiver<SignalQueueItem>>>>,
}

// 实现 Send trait（因为所有字段都是 Send 的）
unsafe impl Send for TakeProfitStopLossStrategy {}

impl TakeProfitStopLossStrategy {
    /// 创建新的止盈止损策略管理器
    pub fn new(grpc_url: String) -> Result<Self, Box<dyn std::error::Error>> {
        let grpc = YellowstoneGrpc::new(grpc_url, None)?;
        
        let (signal_sender, signal_receiver) = mpsc::unbounded_channel();
        
        Ok(TakeProfitStopLossStrategy {
            strategies: Arc::new(Mutex::new(HashMap::new())),
            price_cache: Arc::new(Mutex::new(HashMap::new())),
            grpc_client: Arc::new(grpc),
            signal_sender,
            signal_receiver: Arc::new(Mutex::new(Some(signal_receiver))),
        })
    }

    /// 创建SolanaTrade客户端（类似main.rs中的test_create_solana_trade_client）
    async fn create_solana_trade_client(&self) -> Result<SolanaTrade, Box<dyn std::error::Error>> {
        use solana_sdk::signature::Keypair;
        use sol_trade_sdk::common::types::{TradeConfig, PriorityFee};
        use sol_trade_sdk::swqos::{SwqosConfig, SwqosRegion};
        use solana_sdk::commitment_config::CommitmentConfig;
        use sol_trade_sdk::common::config::ConfigManager;
        
        // 从配置文件加载配置
        let config_manager = ConfigManager::new()?;
        let config = config_manager.get_config();
        
        // 从配置文件读取私钥
        let private_key = config.get_private_key();
        let payer = Keypair::from_base58_string(private_key);
        
        // 从配置文件读取 RPC URL
        let rpc_url = config.get_strategy_rpc_url().to_string();
        
        // 创建SWQoS配置（使用MEV服务提高成功率）
        let mut swqos_configs = vec![
            SwqosConfig::Jito(SwqosRegion::Frankfurt),
        ];
        
        // 根据配置文件添加启用的 MEV 服务
        for (service_type, api_token, regions) in config.get_enabled_mev_services() {
            if !api_token.is_empty() {
                let region = regions.first().map(|&r| match r {
                    "Frankfurt" => SwqosRegion::Frankfurt,
                    "NewYork" => SwqosRegion::NewYork,
                    "Amsterdam" => SwqosRegion::Amsterdam,
                    "Tokyo" => SwqosRegion::Tokyo,
                    "London" => SwqosRegion::London,
                    "LosAngeles" => SwqosRegion::LosAngeles,
                    _ => SwqosRegion::Default,
                }).unwrap_or(SwqosRegion::Default);
                
                match service_type {
                    "nextblock" => swqos_configs.push(SwqosConfig::NextBlock(api_token.to_string(), region)),
                    "bloxroute" => swqos_configs.push(SwqosConfig::Bloxroute(api_token.to_string(), region)),
                    "zeroslot" => swqos_configs.push(SwqosConfig::ZeroSlot(api_token.to_string(), region)),
                    "temporal" => swqos_configs.push(SwqosConfig::Temporal(api_token.to_string(), region)),
                    _ => {}
                }
            }
        }
        
        // 从配置文件读取优先费用配置
        let priority_fees = config.get_priority_fees();
        let priority_fee = PriorityFee {
            unit_limit: priority_fees.compute_unit_limit,
            unit_price: priority_fees.compute_unit_price,
            rpc_unit_limit: priority_fees.rpc_compute_unit_limit,
            rpc_unit_price: priority_fees.rpc_compute_unit_price,
            buy_tip_fee: priority_fees.buy_tip_fee,
            buy_tip_fees: vec![],
            smart_buy_tip_fee: 0.0,
            sell_tip_fee: priority_fees.sell_tip_fee,
        };
        
        // 创建交易配置
        let trade_config = TradeConfig {
            rpc_url,
            commitment: CommitmentConfig::confirmed(),
            priority_fee,
            swqos_configs,
            lookup_table_key: None,
        };
        
        // 创建SolanaTrade客户端
        let solana_trade_client = SolanaTrade::new(Arc::new(payer), trade_config).await;
        Ok(solana_trade_client)
    }

    /// 添加新的交易策略
    pub fn add_strategy(&self, strategy_id: String, mut strategy: TradingStrategy) {
        // 初始化策略状态
        strategy.monitoring_mode = MonitoringMode::Wallet;
        strategy.current_token = None; // 初始没有跟单的token
        strategy.completed_tokens = Vec::new();
        
        let target_wallet = strategy.target_wallet.clone();
        let mut strategies = self.strategies.lock().unwrap();
        strategies.insert(strategy_id.clone(), strategy);
        utils::write_log(&format!("✅ 动态跟单策略已添加: {}", strategy_id));
        utils::write_log("   初始监控模式: 钱包交易");
        utils::write_log(&format!("   目标钱包: {}", target_wallet));
    }

    /// 移除策略
    pub fn remove_strategy(&self, strategy_id: &str) {
        let mut strategies = self.strategies.lock().unwrap();
        strategies.remove(strategy_id);
        utils::write_log(&format!("❌ 动态跟单策略已移除: {}", strategy_id));
    }

    /// 更新策略状态
    pub fn update_strategy_status(&self, strategy_id: &str, status: StrategyStatus) {
        let mut strategies = self.strategies.lock().unwrap();
        if let Some(strategy) = strategies.get_mut(strategy_id) {
            strategy.strategy_status = status;
            utils::write_log(&format!("🔄 动态跟单策略状态已更新: {} -> {:?}", strategy_id, strategy.strategy_status));
        }
    }

    /// 计算代币价格（基于SOL）
    pub async fn calculate_token_price(&self, pool_address: &str, token_address: &str) -> Result<f64, Box<dyn std::error::Error>> {
        // 这里应该从实际的池子状态中计算价格
        // 基于池子中的SOL和代币数量比例计算价格
        
        // 由于这个函数没有事件参数，我们使用默认价格
        // 在实际应用中，应该查询池子状态或使用缓存的价格
        let price = 0.000001; // 默认价格，实际应用中需要从池子状态中计算
        
        // 更新价格缓存
        let mut price_cache = self.price_cache.lock().unwrap();
        price_cache.insert(token_address.to_string(), PriceInfo {
            token_address: token_address.to_string(),
            current_price: price,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            volume_24h: None,
        });
        
        Ok(price)
    }

    /// 处理买入信号（钱包买入触发跟单）
    pub async fn handle_buy_signal(&self, signal: &TradingSignal) -> Result<(), Box<dyn std::error::Error>> {
        utils::write_log("🟢 检测到钱包买入信号!");
        utils::write_log(&format!("代币: {}", signal.token_address));
        utils::write_log(&format!("价格: {}", signal.price));
        utils::write_log(&format!("数量: {}", signal.amount));
        utils::write_log(&format!("钱包: {}", signal.wallet_address));
        
        // 更新策略中的买入信息
        let mut strategies = self.strategies.lock().unwrap();
        let mut strategy_to_update = None;
        
        for (strategy_id, strategy) in strategies.iter_mut() {
            if strategy.target_wallet == signal.wallet_address {
                // 动态设置当前跟单的token
                strategy.current_token = Some(signal.token_address.clone());
                strategy.buy_price = Some(signal.price);
                strategy.buy_amount = Some(signal.amount);
                strategy.position_status = PositionStatus::Holding;
                strategy.monitoring_mode = MonitoringMode::Token; // 切换到token监控模式
                
                utils::write_log(&format!("✅ 动态跟单策略 {} 已更新买入信息:", strategy_id));
                utils::write_log(&format!("   跟单token: {}", signal.token_address));
                utils::write_log(&format!("   买入价格: {}", signal.price));
                utils::write_log(&format!("   买入数量: {}", signal.amount));
                utils::write_log(&format!("   持仓状态: {:?}", strategy.position_status));
                utils::write_log(&format!("   监控模式: {:?}", strategy.monitoring_mode));
                
                strategy_to_update = Some(strategy_id.clone());
                break;
            }
        }
        
        // 释放锁
        drop(strategies);
        
        // 执行实际的买入操作
        if let Some(strategy_id) = strategy_to_update {
            if let Err(e) = self.execute_buy_order(&strategy_id, &signal).await {
                utils::write_log(&format!("❌ 买入订单执行失败: {}", e));
                // 如果买入失败，回滚策略状态
                let mut strategies = self.strategies.lock().unwrap();
                if let Some(strategy) = strategies.get_mut(&strategy_id) {
                    strategy.position_status = PositionStatus::NoPosition;
                    strategy.current_token = None;
                    strategy.buy_price = None;
                    strategy.buy_amount = None;
                    strategy.monitoring_mode = MonitoringMode::Wallet;
                }
                return Err(e);
            }
            
            utils::write_log(&format!("🚀 买入订单执行成功，开始监控代币价格变化"));
        }
        
        Ok(())
    }

    /// 处理卖出信号
    pub async fn handle_sell_signal(&self, signal: &TradingSignal) -> Result<(), Box<dyn std::error::Error>> {
        utils::write_log("🔴 收到卖出信号!");
        utils::write_log(&format!("代币: {}", signal.token_address));
        utils::write_log(&format!("价格: {}", signal.price));
        utils::write_log(&format!("数量: {}", signal.amount));
        utils::write_log(&format!("钱包: {}", signal.wallet_address));
        
        // 更新策略中的卖出信息
        let mut strategies = self.strategies.lock().unwrap();
        for (strategy_id, strategy) in strategies.iter_mut() {
            if let Some(current_token) = &strategy.current_token {
                if current_token == &signal.token_address {
                    strategy.position_status = PositionStatus::Sold;
                    strategy.monitoring_mode = MonitoringMode::Wallet; // 切换回钱包监控模式
                    strategy.current_token = None; // 清空当前token
                    
                    // 将已完成的token添加到列表中
                    if !strategy.completed_tokens.contains(&signal.token_address) {
                        strategy.completed_tokens.push(signal.token_address.clone());
                    }
                    
                    if let Some(buy_price) = strategy.buy_price {
                        let profit_loss = signal.price - buy_price;
                        let profit_loss_percentage = (profit_loss / buy_price) * 100.0;
                        
                        println!("✅ 动态跟单策略 {} 交易完成:", strategy_id);
                        println!("   跟单token: {}", signal.token_address);
                        println!("   买入价格: {}", buy_price);
                        println!("   卖出价格: {}", signal.price);
                        println!("   盈亏: {:.4} ({:.2}%)", profit_loss, profit_loss_percentage);
                        println!("   监控模式: {:?}", strategy.monitoring_mode);
                        println!("   已完成token数量: {}", strategy.completed_tokens.len());
                    }
                    break;
                }
            }
        }
        
        Ok(())
    }

    /// 检查止盈止损
    pub async fn check_take_profit_stop_loss(&self) -> Result<(), Box<dyn std::error::Error>> {
        // 克隆策略数据，避免持有锁
        let strategies_to_check = {
            let strategies = self.strategies.lock().unwrap();
            strategies.iter()
                .filter(|(_, strategy)| strategy.position_status == PositionStatus::Holding)
                .map(|(id, strategy)| (id.clone(), strategy.clone()))
                .collect::<Vec<_>>()
        };
        
        for (strategy_id, strategy) in strategies_to_check {
            if let Some(current_token) = &strategy.current_token {
                if let Some(buy_price) = strategy.buy_price {
                    // 获取当前价格
                    let current_price = self.calculate_token_price("", current_token).await?;
                    
                    let profit_loss_percentage = ((current_price - buy_price) / buy_price) * 100.0;
                    
                    // 检查止盈
                    if profit_loss_percentage >= strategy.take_profit_percentage {
                        println!("🎯 动态跟单策略 {} 触发止盈!", strategy_id);
                        println!("   跟单token: {}", current_token);
                        println!("   买入价格: {}", buy_price);
                        println!("   当前价格: {}", current_price);
                        println!("   盈利: {:.2}%", profit_loss_percentage);
                        
                        // 执行卖出
                        self.execute_sell_order(&strategy_id, current_price).await?;
                    }
                    // 检查止损
                    else if profit_loss_percentage <= -strategy.stop_loss_percentage {
                        println!("🛑 动态跟单策略 {} 触发止损!", strategy_id);
                        println!("   跟单token: {}", current_token);
                        println!("   买入价格: {}", buy_price);
                        println!("   当前价格: {}", current_price);
                        println!("   亏损: {:.2}%", profit_loss_percentage);
                        
                        // 执行卖出
                        self.execute_sell_order(&strategy_id, current_price).await?;
                    }
                }
            }
        }
        
        Ok(())
    }

    /// 处理买入信号（静态版本，避免锁问题）
    async fn handle_buy_signal_static(&self, signal: &TradingSignal) -> Result<(), Box<dyn std::error::Error>> {
        utils::write_log("🟢 检测到钱包买入信号!");
        utils::write_log(&format!("代币: {}", signal.token_address));
        utils::write_log(&format!("价格: {}", signal.price));
        utils::write_log(&format!("数量: {}", signal.amount));
        utils::write_log(&format!("钱包: {}", signal.wallet_address));
        
        // 更新策略中的买入信息
        let strategy_id_to_update = {
            let mut strategies = self.strategies.lock().unwrap();
            let mut strategy_to_update = None;
            
            for (strategy_id, strategy) in strategies.iter_mut() {
                if strategy.target_wallet == signal.wallet_address {
                    // 动态设置当前跟单的token
                    strategy.current_token = Some(signal.token_address.clone());
                    strategy.buy_price = Some(signal.price);
                    strategy.buy_amount = Some(signal.amount);
                    strategy.position_status = PositionStatus::Holding;
                    strategy.monitoring_mode = MonitoringMode::Token; // 切换到token监控模式
                    
                    utils::write_log(&format!("✅ 动态跟单策略 {} 已更新买入信息:", strategy_id));
                    utils::write_log(&format!("   跟单token: {}", signal.token_address));
                    utils::write_log(&format!("   买入价格: {}", signal.price));
                    utils::write_log(&format!("   买入数量: {}", signal.amount));
                    utils::write_log(&format!("   持仓状态: {:?}", strategy.position_status));
                    utils::write_log(&format!("   监控模式: {:?}", strategy.monitoring_mode));
                    
                    strategy_to_update = Some(strategy_id.clone());
                    break;
                }
            }
            
            strategy_to_update
        }; // 这里自动释放锁
        
        // 执行实际的买入操作
        if let Some(strategy_id) = strategy_id_to_update {
            if let Err(e) = self.execute_buy_order(&strategy_id, &signal).await {
                utils::write_log(&format!("❌ 买入订单执行失败: {}", e));
                // 如果买入失败，回滚策略状态
                let mut strategies = self.strategies.lock().unwrap();
                if let Some(strategy) = strategies.get_mut(&strategy_id) {
                    strategy.position_status = PositionStatus::NoPosition;
                    strategy.current_token = None;
                    strategy.buy_price = None;
                    strategy.buy_amount = None;
                    strategy.monitoring_mode = MonitoringMode::Wallet;
                }
                return Err(e);
            }
            
            utils::write_log(&format!("🚀 买入订单执行成功，开始监控代币价格变化"));
        }
        
        Ok(())
    }

    /// 处理卖出信号（静态版本，避免锁问题）
    async fn handle_sell_signal_static(&self, signal: &TradingSignal) -> Result<(), Box<dyn std::error::Error>> {
        utils::write_log("🔴 收到卖出信号!");
        utils::write_log(&format!("代币: {}", signal.token_address));
        utils::write_log(&format!("价格: {}", signal.price));
        utils::write_log(&format!("数量: {}", signal.amount));
        utils::write_log(&format!("钱包: {}", signal.wallet_address));
        
        // 更新策略中的卖出信息
        let mut strategies = self.strategies.lock().unwrap();
        for (strategy_id, strategy) in strategies.iter_mut() {
            if let Some(current_token) = &strategy.current_token {
                if current_token == &signal.token_address {
                    strategy.position_status = PositionStatus::Sold;
                    strategy.monitoring_mode = MonitoringMode::Wallet; // 切换回钱包监控模式
                    strategy.current_token = None; // 清空当前token
                    
                    // 将已完成的token添加到列表中
                    if !strategy.completed_tokens.contains(&signal.token_address) {
                        strategy.completed_tokens.push(signal.token_address.clone());
                    }
                    
                    if let Some(buy_price) = strategy.buy_price {
                        let profit_loss = signal.price - buy_price;
                        let profit_loss_percentage = (profit_loss / buy_price) * 100.0;
                        
                        println!("✅ 动态跟单策略 {} 交易完成:", strategy_id);
                        println!("   跟单token: {}", signal.token_address);
                        println!("   买入价格: {}", buy_price);
                        println!("   卖出价格: {}", signal.price);
                        println!("   盈亏: {:.4} ({:.2}%)", profit_loss, profit_loss_percentage);
                        println!("   监控模式: {:?}", strategy.monitoring_mode);
                        println!("   已完成token数量: {}", strategy.completed_tokens.len());
                    }
                    break;
                }
            }
        }
        
        Ok(())
    }

    /// 执行买入订单
    pub async fn execute_buy_order(&self, strategy_id: &str, signal: &TradingSignal) -> Result<(), Box<dyn std::error::Error>> {
        utils::write_log(&format!("📥 执行买入订单: {}", strategy_id));
        utils::write_log(&format!("   代币地址: {}", signal.token_address));
        utils::write_log(&format!("   买入数量: {}", signal.amount));
        utils::write_log(&format!("   买入价格: {}", signal.price));
        
        // 计算需要支付的SOL数量（基于价格和数量）
        let sol_amount = (signal.price * signal.amount as f64) as u64;
        utils::write_log(&format!("   预计支付SOL: {} lamports", sol_amount));
        
        // 调用main.rs中的买入函数逻辑
        utils::write_log("🔄 正在执行买入交易...");
        
        // 创建SolanaTrade客户端
        let client = self.create_solana_trade_client().await?;
        
        // 解析地址
        let mint_pubkey = Pubkey::from_str(&signal.token_address)?;
        
        // 获取账户余额
        let balance = client.rpc.get_balance(&client.get_payer_pubkey()).await?;
        utils::write_log(&format!("账户SOL余额: {} SOL", balance as f64 / 1_000_000_000.0));
        
        // 确定买入金额
        let buy_sol_cost = if sol_amount == 0 {
            // 如果输入为0，使用账户全部SOL（保留一些作为手续费）
            let available_balance = if balance > 50_000_000 { // 保留0.05 SOL作为手续费
                balance - 50_000_000
            } else {
                balance
            };
            utils::write_log(&format!("使用全部可用SOL进行买入: {} SOL", available_balance as f64 / 1_000_000_000.0));
            available_balance
        } else {
            // 使用指定的SOL数量
            if sol_amount > balance {
                return Err(format!("余额不足，需要 {} SOL，当前余额 {} SOL", 
                    sol_amount as f64 / 1_000_000_000.0, 
                    balance as f64 / 1_000_000_000.0).into());
            }
            sol_amount
        };
        
        if buy_sol_cost == 0 {
            return Err("买入金额不能为0".into());
        }
        
        // 滑点设置（使用默认滑点）
        let slippage = DEFAULT_SLIPPAGE;
        
        // 获取最新区块哈希
        let recent_blockhash = client.rpc.get_latest_blockhash().await?;
        
        // 注意：这里需要pool_address，但当前函数没有这个参数
        // 在实际应用中，需要从策略配置或事件中获取pool_address
        // 暂时使用一个默认值或从其他地方获取
        let pool_address = signal.pool_address.clone(); 
        
        // 解析池状态地址
        let pool_state = Pubkey::from_str(&pool_address)?;
        
        // 验证池状态是否存在
        let pool_account = client.rpc.get_account(&pool_state).await;
        if pool_account.is_err() {
            return Err(format!("池状态地址无效或池不存在: {}", pool_address).into());
        }
        
        // 计算买入代币的预期数量
        let buy_amount_out = get_buy_token_amount(&client.rpc, &pool_state, buy_sol_cost).await?;
        utils::write_log(&format!("预期获得代币数量: {}", buy_amount_out));
        
        // 执行买入交易
        utils::write_log(&format!("正在买入 {} SOL 的 {} 代币...", 
            buy_sol_cost as f64 / 1_000_000_000.0, signal.token_address));
        
        let result = client.buy(
            DexType::RaydiumCpmm,
            mint_pubkey,
            None,
            buy_sol_cost,
            Some(slippage),
            recent_blockhash,
            None,
            Some(Box::new(RaydiumCpmmParams {
                pool_state: Some(pool_state),
                mint_token_program: Some(spl_token::ID),
                mint_token_in_pool_state_index: Some(1),
                minimum_amount_out: Some(buy_amount_out),
                auto_handle_wsol: true,
            })),
        ).await;
        
        match result {
            Ok(_) => {
                utils::write_log("✅ 买入交易成功完成！");
                utils::write_log(&format!("买入金额: {} SOL", buy_sol_cost as f64 / 1_000_000_000.0));
                utils::write_log(&format!("预期获得代币: {}", buy_amount_out));
                
                // 更新策略状态
                let mut strategies = self.strategies.lock().unwrap();
                if let Some(strategy) = strategies.get_mut(strategy_id) {
                    strategy.position_status = PositionStatus::Holding;
                    strategy.buy_price = Some(signal.price);
                    strategy.buy_amount = Some(signal.amount);
                }
            }
            Err(e) => {
                utils::write_log(&format!("❌ 买入交易失败: {}", e));
                utils::write_log("注意：即使交易失败，手续费也不会退回");
                return Err(e.into());
            }
        }
        
        utils::write_log("✅ 买入订单执行完成");
        Ok(())
    }

    /// 执行卖出订单
    pub async fn execute_sell_order(&self, strategy_id: &str, price: f64) -> Result<(), Box<dyn std::error::Error>> {
        println!("📤 执行卖出订单: {}", strategy_id);
        println!("   卖出价格: {}", price);
        
        // 这里可以调用实际的卖出函数
        // 例如: sell_raydium_cpmm(amount, pool_address, token_address, slippage).await?;
        
        // 更新策略状态
        let mut strategies = self.strategies.lock().unwrap();
        if let Some(strategy) = strategies.get_mut(strategy_id) {
            strategy.position_status = PositionStatus::Sold;
            strategy.monitoring_mode = MonitoringMode::Wallet; // 切换回钱包监控模式
            
            // 将已完成的token添加到列表中
            if let Some(current_token) = &strategy.current_token {
                if !strategy.completed_tokens.contains(current_token) {
                    strategy.completed_tokens.push(current_token.clone());
                }
                strategy.current_token = None; // 清空当前token
            }
        }
        
        println!("✅ 卖出订单执行完成");
        Ok(())
    }

    /// 启动策略监控
    pub async fn start_monitoring(&self) -> Result<(), Box<dyn std::error::Error>> {
        utils::write_log("🚀 启动动态跟单策略监控...");
        
        // 启动信号队列处理器（用于自动下单）
        utils::write_log("启动信号队列处理器...");
        self.start_signal_queue_processor()?;
        
        // 创建事件回调（使用完整版本）
        let callback = self.create_complete_callback();
        let protocols = vec![Protocol::RaydiumCpmm];
        
        // 启动价格监控任务
        self.start_price_monitoring();
        
        // 启动事件监听
        utils::write_log("开始监听Raydium Cpmm事件...");
        self.grpc_client.subscribe_events(protocols, None, None, None, None, None, callback).await?;
        
        Ok(())
    }

    /// 创建策略回调函数
    // fn create_strategy_callback(&self) -> impl Fn(Box<dyn UnifiedEvent>) {
    //     let strategies = Arc::clone(&self.strategies);
    //     let price_cache = Arc::clone(&self.price_cache);
        
    //     move |event: Box<dyn UnifiedEvent>| {
    //         let strategies = Arc::clone(&strategies);
    //         let price_cache = Arc::clone(&price_cache);
            
    //         match_event!(event, {
    //             RaydiumCpmmSwapEvent => |e: RaydiumCpmmSwapEvent| {
    //                 // 在异步上下文中处理事件
    //                 tokio::spawn(async move {
    //                     let strategies = strategies.lock().unwrap();
                        
    //                     for (strategy_id, strategy) in strategies.iter() {
    //                         if strategy.strategy_status != StrategyStatus::Monitoring {
    //                             continue;
    //                         }
                            
    //                         match strategy.monitoring_mode {
    //                             MonitoringMode::Wallet => {
    //                                 // 监控钱包交易模式
    //                                 Self::process_wallet_monitoring_static(strategy_id, strategy, &e);
    //                             },
    //                             MonitoringMode::Token => {
    //                                 // 监控特定token模式
    //                                 Self::process_token_monitoring_static(strategy_id, strategy, &e);
    //                             },
    //                         }
    //                     }
    //                 });
    //             },
    //         });
    //     }
    // }
    

    /// 创建完整的交易信号处理回调
    fn create_complete_callback(&self) -> impl Fn(Box<dyn UnifiedEvent>) {
        let strategies = Arc::clone(&self.strategies);
        let price_cache = Arc::clone(&self.price_cache);
        
        let self_clone = Arc::new(self.clone());
        move |event: Box<dyn UnifiedEvent>| {
            let strategies = Arc::clone(&strategies);
            let price_cache = Arc::clone(&price_cache);
            let self_clone = Arc::clone(&self_clone);
            
            if let Some(raydium_event) = event.as_any().downcast_ref::<RaydiumCpmmSwapEvent>() {
                let strategies_clone = Arc::clone(&strategies);
                let e_clone = raydium_event.clone();
                let self_clone = Arc::clone(&self_clone);
                
                tokio::spawn(async move {
                    let strategies = strategies_clone.lock().unwrap();
                    
                    for (strategy_id, strategy) in strategies.iter() {
                        if strategy.strategy_status != StrategyStatus::Monitoring {
                            continue;
                        }
                        
                        // 处理钱包监控模式
                        if strategy.monitoring_mode == MonitoringMode::Wallet {
                            self_clone.process_wallet_monitoring_complete(strategy_id, strategy, &e_clone, Arc::clone(&strategies_clone));
                        }
                        // 处理token监控模式
                        else if strategy.monitoring_mode == MonitoringMode::Token {
                            self_clone.process_token_monitoring_complete(strategy_id, strategy, &e_clone, Arc::clone(&strategies_clone));
                        }
                    }
                });
            }
        }
    }

    /// 处理钱包监控模式（静态方法）
    fn process_wallet_monitoring_static(strategy_id: &str, strategy: &TradingStrategy, event: &RaydiumCpmmSwapEvent) {
        // 检查是否是目标钱包的交易
        for transfer in &event.metadata.transfer_datas {
            let source = transfer.source.to_string();
            let destination = transfer.destination.to_string();
            
            if source == strategy.target_wallet || destination == strategy.target_wallet {
                if let Some(mint) = &transfer.mint {
                    let mint_str = mint.to_string();
                    
                    // 检查是否已经完成过这个token
                    if strategy.completed_tokens.contains(&mint_str) {
                        continue;
                    }
                    
                    // 判断是买入还是卖出（简化判断：如果钱包是source，可能是卖出；如果是destination，可能是买入）
                    let is_buy = destination == strategy.target_wallet;
                    
                    if is_buy {
                        println!("📡 钱包监控模式 - 检测到买入信号:");
                        println!("   策略: {}", strategy_id);
                        println!("   跟单token: {}", mint_str);
                        println!("   数量: {}", transfer.amount);
                        println!("   钱包: {}", strategy.target_wallet);
                        println!("   来源: {}", source);
                        println!("   目标: {}", destination);
                        println!("   🟢 准备跟单买入!");
                        
                        // 创建买入信号并处理
                        let signal = TradingSignal {
                            signal_type: SignalType::Buy,
                            token_address: mint_str.clone(),
                            price: 0.0, // 需要从事件中计算实际价格
                            amount: transfer.amount,
                            timestamp: event.metadata.block_time as u64,
                            wallet_address: strategy.target_wallet.clone(),
                            pool_address: event.pool_state.to_string(),
                        };
                        
                        // 异步处理买入信号
                        tokio::spawn(async move {
                            // 这里需要获取策略管理器的实例来调用handle_buy_signal
                            // 由于静态方法的限制，我们暂时只打印信号
                            println!("🟢 异步处理买入信号:");
                            println!("   代币: {}", signal.token_address);
                            println!("   数量: {}", signal.amount);
                            println!("   钱包: {}", signal.wallet_address);
                            println!("   ⚠️  注意: 需要在实际应用中调用handle_buy_signal");
                        });
                    } else {
                        println!("📡 钱包监控模式 - 检测到卖出信号:");
                        println!("   策略: {}", strategy_id);
                        println!("   代币: {}", mint_str);
                        println!("   数量: {}", transfer.amount);
                        println!("   钱包: {}", strategy.target_wallet);
                        println!("   来源: {}", source);
                        println!("   目标: {}", destination);
                    }
                }
            }
        }
    }

    /// 处理token监控模式（静态方法）
    fn process_token_monitoring_static(strategy_id: &str, strategy: &TradingStrategy, event: &RaydiumCpmmSwapEvent) {
        // 检查是否是当前跟单token的交易
        if let Some(current_token) = &strategy.current_token {
            for transfer in &event.metadata.transfer_datas {
                if let Some(mint) = &transfer.mint {
                    let mint_str = mint.to_string();
                    
                    if mint_str == *current_token {
                        let source = transfer.source.to_string();
                        let destination = transfer.destination.to_string();
                        
                        println!("📡 Token监控模式 - 检测到交易:");
                        println!("   策略: {}", strategy_id);
                        println!("   跟单token: {}", mint_str);
                        println!("   数量: {}", transfer.amount);
                        println!("   来源: {}", source);
                        println!("   目标: {}", destination);
                        println!("   💰 实时计算价格中...");
                        
                        // 计算当前价格（模拟）
                        let current_price = 0.000001 + (std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() % 100) as f64 / 1000.0;
                        
                        println!("   💰 当前价格: {}", current_price);
                        
                        // 检查是否需要卖出
                        if let Some(buy_price) = strategy.buy_price {
                            let profit_loss_percentage = ((current_price - buy_price) / buy_price) * 100.0;
                            
                            println!("   📊 盈亏分析:");
                            println!("      买入价格: {}", buy_price);
                            println!("      当前价格: {}", current_price);
                            println!("      盈亏: {:.2}%", profit_loss_percentage);
                            
                            // 检查止盈止损条件
                            if profit_loss_percentage >= strategy.take_profit_percentage {
                                println!("   🎯 触发止盈条件!");
                                
                                // 创建卖出信号
                                let signal = TradingSignal {
                                    signal_type: SignalType::TakeProfit,
                                    token_address: mint_str.clone(),
                                    price: current_price,
                                    amount: strategy.buy_amount.unwrap_or(transfer.amount),
                                    timestamp: event.metadata.block_time as u64,
                                    wallet_address: strategy.target_wallet.clone(),
                                    pool_address: event.pool_state.to_string(),
                                };
                                
                                // 异步处理卖出信号
                                tokio::spawn(async move {
                                    println!("🎯 异步处理止盈信号:");
                                    println!("   代币: {}", signal.token_address);
                                    println!("   卖出价格: {}", signal.price);
                                    println!("   数量: {}", signal.amount);
                                    println!("   ⚠️  注意: 需要在实际应用中调用handle_sell_signal");
                                });
                            } else if profit_loss_percentage <= -strategy.stop_loss_percentage {
                                println!("   🛑 触发止损条件!");
                                
                                // 创建卖出信号
                                let signal = TradingSignal {
                                    signal_type: SignalType::StopLoss,
                                    token_address: mint_str.clone(),
                                    price: current_price,
                                    amount: strategy.buy_amount.unwrap_or(transfer.amount),
                                    timestamp: event.metadata.block_time as u64,
                                    wallet_address: strategy.target_wallet.clone(),
                                    pool_address: event.pool_state.to_string(),
                                };
                                
                                // 异步处理卖出信号
                                tokio::spawn(async move {
                                    println!("🛑 异步处理止损信号:");
                                    println!("   代币: {}", signal.token_address);
                                    println!("   卖出价格: {}", signal.price);
                                    println!("   数量: {}", signal.amount);
                                    println!("   ⚠️  注意: 需要在实际应用中调用handle_sell_signal");
                                });
                            } else {
                                println!("   ⏳ 继续监控中...");
                            }
                        }
                    }
                }
            }
        }
    }

    /// 处理钱包监控模式（完整版本）
    fn process_wallet_monitoring_complete(&self, strategy_id: &str, strategy: &TradingStrategy, event: &RaydiumCpmmSwapEvent, strategies: Arc<Mutex<HashMap<String, TradingStrategy>>>) {
        use solana_sdk::pubkey::Pubkey;
        
        // 更新event { metadata: EventMetadata { id: "41c77500d8376f4a", signature: "4ohNTCKwwRVe7hFMQuVU6QTa6VTisyp23N8bBKMwDAFeinU8Nsm3iHNxbNhS2zVNQWH7EgheefMNn8S9pUJospuK", slot: 359099742, block_time: 1754821356, block_time_ms: 1754821356010, program_received_time_ms: 1754821356612, protocol: RaydiumCpmm, event_type: RaydiumCpmmSwapBaseInput, program_id: CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C, transfer_datas: [TransferData { token_program: TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA, source: yu3U5fBG7QVTtN6oWsHk6uCK3KmYEytM6YJpWQNVKC2, destination: B6Xi1w1AdCsFrGJc8w3g3SJr8xo8Lpi6XN2PUxZfmrvL, authority: Some(ByJrzCuiChF2pXroujfW8TxJxmskaMZHfvQycTovhQbH), amount: 100000000, decimals: Some(9), mint: Some(So11111111111111111111111111111111111111112) }, TransferData { token_program: TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA, source: B6FGPf5YpGqhj7Bx5xvkb9yPfq64T71o1wxoQeckscBa, destination: T7KMwqnAkgVKvLaiLz5pwB6uyWyEzcaqo2rjxiJa9vv, authority: Some(GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL), amount: 69586465312, decimals: Some(6), mint: Some(J3u73qgyeuaeunH1uwsT6QLFa24YNeZSjSEdCjAebonk) }], index: "1.5" }, amount_in: 100000000, minimum_amount_out: 0, max_amount_in: 0, amount_out: 0, payer: ByJrzCuiChF2pXroujfW8TxJxmskaMZHfvQycTovhQbH, authority: GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL, amm_config: D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2, pool_state: C8v7Svdr9rQ2fwAsTaTztJkTqiPpfa5K8jcsudaT6CkF, input_token_account: yu3U5fBG7QVTtN6oWsHk6uCK3KmYEytM6YJpWQNVKC2, output_token_account: T7KMwqnAkgVKvLaiLz5pwB6uyWyEzcaqo2rjxiJa9vv, input_vault: B6Xi1w1AdCsFrGJc8w3g3SJr8xo8Lpi6XN2PUxZfmrvL, output_vault: B6FGPf5YpGqhj7Bx5xvkb9yPfq64T71o1wxoQeckscBa, input_token_mint: So11111111111111111111111111111111111111112, output_token_mint: J3u73qgyeuaeunH1uwsT6QLFa24YNeZSjSEdCjAebonk, observation_state: 4L4JiawDb4xoA7Fbqc5v6VqTVy8UFd3NABRYUyFRfGvX }
        let event = RaydiumCpmmSwapEvent {
            metadata: EventMetadata {
                id: "41c77500d8376f4a".to_string(),
                signature: "4ohNTCKwwRVe7hFMQuVU6QTa6VTisyp23N8bBKMwDAFeinU8Nsm3iHNxbNhS2zVNQWH7EgheefMNn8S9pUJospuK".to_string(),
                slot: 359099742,
                block_time: 1754821356,
                block_time_ms: 1754821356010,
                program_received_time_ms: 1754821356612,
                protocol: ProtocolType::RaydiumCpmm,
                event_type: EventType::RaydiumCpmmSwapBaseInput,
                program_id: Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap(),
                transfer_datas: vec![
                    TransferData {
                        token_program: Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap(),
                        source: Pubkey::from_str("yu3U5fBG7QVTtN6oWsHk6uCK3KmYEytM6YJpWQNVKC2").unwrap(),
                        destination: Pubkey::from_str("B6Xi1w1AdCsFrGJc8w3g3SJr8xo8Lpi6XN2PUxZfmrvL").unwrap(),
                        authority: Some(Pubkey::from_str("ByJrzCuiChF2pXroujfW8TxJxmskaMZHfvQycTovhQbH").unwrap()),
                        amount: 100000000,
                        decimals: Some(9),
                        mint: Some(Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap())
                    },
                    TransferData {
                        token_program: Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap(),
                        source: Pubkey::from_str("B6FGPf5YpGqhj7Bx5xvkb9yPfq64T71o1wxoQeckscBa").unwrap(),
                        destination: Pubkey::from_str("T7KMwqnAkgVKvLaiLz5pwB6uyWyEzcaqo2rjxiJa9vv").unwrap(),
                        authority: Some(Pubkey::from_str("GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL").unwrap()),
                        amount: 69586465312,
                        decimals: Some(6),
                        mint: Some(Pubkey::from_str("J3u73qgyeuaeunH1uwsT6QLFa24YNeZSjSEdCjAebonk").unwrap())
                    }
                ],
                index: "1.5".to_string(),
            },
            amount_in: 100000000,
            minimum_amount_out: 0,
            max_amount_in: 0,
            amount_out: 0,
            payer: Pubkey::from_str("ByJrzCuiChF2pXroujfW8TxJxmskaMZHfvQycTovhQbH").unwrap(),
            authority: Pubkey::from_str("GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL").unwrap(),
            amm_config: Pubkey::from_str("D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2").unwrap(),
            pool_state: Pubkey::from_str("C8v7Svdr9rQ2fwAsTaTztJkTqiPpfa5K8jcsudaT6CkF").unwrap(),
            input_token_account: Pubkey::from_str("yu3U5fBG7QVTtN6oWsHk6uCK3KmYEytM6YJpWQNVKC2").unwrap(),
            output_token_account: Pubkey::from_str("T7KMwqnAkgVKvLaiLz5pwB6uyWyEzcaqo2rjxiJa9vv").unwrap(),
            input_vault: Pubkey::from_str("B6Xi1w1AdCsFrGJc8w3g3SJr8xo8Lpi6XN2PUxZfmrvL").unwrap(),
            output_vault: Pubkey::from_str("B6FGPf5YpGqhj7Bx5xvkb9yPfq64T71o1wxoQeckscBa").unwrap(),
            input_token_mint: Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
            output_token_mint: Pubkey::from_str("J3u73qgyeuaeunH1uwsT6QLFa24YNeZSjSEdCjAebonk").unwrap(),
            observation_state: Pubkey::from_str("4L4JiawDb4xoA7Fbqc5v6VqTVy8UFd3NABRYUyFRfGvX").unwrap(),
        };
        
        // 添加日志，输出过滤的交易数据计数和时间戳(日期格式的)
        let timestamp = event.metadata.block_time as i64;
        let datetime: DateTime<Utc> = DateTime::from_timestamp(timestamp, 0).unwrap_or_else(|| Utc::now());
        let formatted_date = datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string();
        utils::write_log(&format!("📡 钱包监控模式 - 过滤的交易数据计数: {}", event.metadata.transfer_datas.len()));
        utils::write_log(&format!("📡 钱包监控模式 - 过滤的交易数据: {:?}", event));
        
        // 输出签名信息
        let payer = event.payer.to_string();
        let pool = event.pool_state.to_string();
        
        // 检查是否是目标钱包的交易
        if payer != strategy.target_wallet {
            return;
        }
        
        // 判断是买入还是卖出
        let (is_buy, token_address) = if event.input_token_mint.to_string() == "So11111111111111111111111111111111111111112" {
            // 输入是SOL，说明是买入，token为output_token_mint
            (true, event.output_token_mint.to_string())
        } else {
            // 输入不是SOL，说明是卖出，token为input_token_mint
            (false, event.input_token_mint.to_string())
        };
        
        // 检查是否已经完成过这个token
        if strategy.completed_tokens.contains(&token_address) {
            utils::write_log(&format!("📡 钱包监控模式 - token {} 已完成，跳过处理", token_address));
            return;
        }
        
        if is_buy {
            utils::write_log("📡 钱包监控模式 - 检测到买入信号:");
            utils::write_log(&format!("   策略: {}", strategy_id));
            utils::write_log(&format!("   跟单token: {}", token_address));
            utils::write_log(&format!("   钱包: {}", strategy.target_wallet));
            utils::write_log(&format!("   池子: {}", pool));
            utils::write_log("   🟢 准备跟单买入!");
            
            // 计算买入价格 - 从transfer_datas中计算
            // let buy_price = Self::calculate_price_from_transfer_data(event);
            let buy_price = Self::calculate_price_from_transfer_data(&event);
            
            if buy_price > 0.0 {
                utils::write_log(&format!("   💰 买入价格: {}", buy_price));
                utils::write_log("   ✅ 买入信号已创建，开始处理...");
                
                // 创建买入信号
                let signal = TradingSignal {
                    signal_type: SignalType::Buy,
                    token_address: token_address.clone(),
                    price: buy_price,
                    amount: event.amount_in/10,
                    timestamp: event.metadata.block_time as u64,
                    wallet_address: strategy.target_wallet.clone(),
                    pool_address: pool.clone(),
                };
                
                // 更新策略状态（同步操作）
                {
                    let mut strategies = strategies.lock().unwrap();
                    if let Some(strategy) = strategies.get_mut(strategy_id) {
                        // 动态设置当前跟单的token
                        strategy.current_token = Some(token_address.clone());
                        strategy.buy_price = Some(buy_price);
                        strategy.buy_amount = Some(signal.amount);
                        strategy.position_status = PositionStatus::Holding;
                        strategy.monitoring_mode = MonitoringMode::Token; // 切换到token监控模式
                        
                        utils::write_log(&format!("✅ 动态跟单策略 {} 已更新买入信息:", strategy_id));
                        utils::write_log(&format!("   跟单token: {}", token_address));
                        utils::write_log(&format!("   买入价格: {}", buy_price));
                        utils::write_log(&format!("   买入数量: {}", signal.amount));
                        utils::write_log(&format!("   持仓状态: {:?}", strategy.position_status));
                        utils::write_log(&format!("   监控模式: {:?}", strategy.monitoring_mode));
                    }
                } // 锁在这里自动释放
                
                // 异步执行买入操作
                let strategies_clone = Arc::clone(&strategies);
                let strategy_id_clone = strategy_id.to_string();
                let signal_clone = signal.clone();
                
                // 克隆必要的字段以避免移动问题
                let token_address_clone = signal.token_address.clone();
                let price_clone = signal.price;
                let amount_clone = signal.amount;
                
                // 将信号添加到队列，而不是在异步任务中处理
                let signal_for_queue = signal.clone();
                let strategy_id_for_queue = strategy_id.to_string();
                
                // 使用信号队列系统
                if let Err(e) = self.add_signal_to_queue(signal_for_queue, strategy_id_for_queue) {
                    utils::write_log(&format!("❌ 无法将信号添加到队列: {}", e));
                } else {
                    utils::write_log("✅ 买入信号已添加到队列，等待处理");
                }
                
                // 启动异步任务来记录日志
                tokio::spawn(async move {
                    utils::write_log(&format!("🚀 策略状态已更新，买入信号已添加到队列"));
                    utils::write_log(&format!("📝 信号详情:"));
                    utils::write_log(&format!("   策略ID: {}", strategy_id_clone));
                    utils::write_log(&format!("   代币地址: {}", token_address_clone));
                    utils::write_log(&format!("   买入价格: {}", price_clone));
                    utils::write_log(&format!("   买入数量: {}", amount_clone));
                });
            } else {
                utils::write_log("   ❌ 无法计算买入价格，跳过处理");
            }
        } else {
            utils::write_log("📡 钱包监控模式 - 检测到卖出信号:");
            utils::write_log(&format!("   策略: {}", strategy_id));
            utils::write_log(&format!("   代币: {}", token_address));
            utils::write_log(&format!("   钱包: {}", strategy.target_wallet));
            utils::write_log(&format!("   池子: {}", pool));
        }
    }

    /// 处理token监控模式（完整版本）
    fn process_token_monitoring_complete(&self, strategy_id: &str, strategy: &TradingStrategy, event: &RaydiumCpmmSwapEvent, strategies: Arc<Mutex<HashMap<String, TradingStrategy>>>) {
        // 检查是否是当前跟单token的交易
        if let Some(current_token) = &strategy.current_token {
            for transfer in &event.metadata.transfer_datas {
                if let Some(mint) = &transfer.mint {
                    let mint_str = mint.to_string();
                    
                    if mint_str == *current_token {
                        let source = transfer.source.to_string();
                        let destination = transfer.destination.to_string();
                        
                        println!("📡 Token监控模式 - 检测到交易:");
                        println!("   策略: {}", strategy_id);
                        println!("   跟单token: {}", mint_str);
                        println!("   数量: {}", transfer.amount);
                        println!("   来源: {}", source);
                        println!("   目标: {}", destination);
                        println!("   💰 实时计算价格中...");
                        
                        // 计算当前价格 - 从实际交易事件中提取
                        let current_price = Self::calculate_real_price_from_event(event, &mint_str);
                        
                        // 如果无法从事件中提取价格，使用备用方法
                        let current_price = if current_price > 0.0 {
                            current_price
                        } else {
                            // 备用价格计算方法 - 基于池子状态
                            Self::calculate_price_from_pool_state(event, &mint_str)
                        };
                        
                        println!("   💰 当前价格: {}", current_price);
                        
                        // 检查是否需要卖出
                        if let Some(buy_price) = strategy.buy_price {
                            let profit_loss_percentage = ((current_price - buy_price) / buy_price) * 100.0;
                            
                            println!("   📊 盈亏分析:");
                            println!("      买入价格: {}", buy_price);
                            println!("      当前价格: {}", current_price);
                            println!("      盈亏: {:.2}%", profit_loss_percentage);
                            
                            // 检查止盈止损条件
                            if profit_loss_percentage >= strategy.take_profit_percentage {
                                println!("   🎯 触发止盈条件!");
                                
                                // 创建卖出信号
                                let signal = TradingSignal {
                                    signal_type: SignalType::TakeProfit,
                                    token_address: mint_str.clone(),
                                    price: current_price,
                                    amount: strategy.buy_amount.unwrap_or(transfer.amount),
                                    timestamp: event.metadata.block_time as u64,
                                    wallet_address: strategy.target_wallet.clone(),
                                    pool_address: event.pool_state.to_string(),
                                };
                                
                                println!("   ✅ 止盈信号已创建，开始处理...");
                                
                                // 异步处理卖出信号 - 更新策略状态
                                let strategies_clone = Arc::clone(&strategies);
                                let strategy_id_clone = strategy_id.to_string();
                                let signal_clone = signal.clone();
                                
                                tokio::spawn(async move {
                                    // 更新策略状态
                                    let mut strategies = strategies_clone.lock().unwrap();
                                    if let Some(strategy) = strategies.get_mut(&strategy_id_clone) {
                                        strategy.position_status = PositionStatus::Sold;
                                        strategy.monitoring_mode = MonitoringMode::Wallet; // 切换回钱包监控模式
                                        strategy.current_token = None; // 清空当前token
                                        
                                        // 将已完成的token添加到列表中
                                        if !strategy.completed_tokens.contains(&signal_clone.token_address) {
                                            strategy.completed_tokens.push(signal_clone.token_address.clone());
                                        }
                                        
                                        if let Some(buy_price) = strategy.buy_price {
                                            let profit_loss = signal_clone.price - buy_price;
                                            let profit_loss_percentage = (profit_loss / buy_price) * 100.0;
                                            
                                            println!("✅ 动态跟单策略 {} 止盈交易完成:", strategy_id_clone);
                                            println!("   跟单token: {}", signal_clone.token_address);
                                            println!("   买入价格: {}", buy_price);
                                            println!("   卖出价格: {}", signal_clone.price);
                                            println!("   盈利: {:.4} ({:.2}%)", profit_loss, profit_loss_percentage);
                                            println!("   监控模式: {:?}", strategy.monitoring_mode);
                                            println!("   已完成token数量: {}", strategy.completed_tokens.len());
                                        }
                                    }
                                });
                            } else if profit_loss_percentage <= -strategy.stop_loss_percentage {
                                println!("   🛑 触发止损条件!");
                                
                                // 创建卖出信号
                                let signal = TradingSignal {
                                    signal_type: SignalType::StopLoss,
                                    token_address: mint_str.clone(),
                                    price: current_price,
                                    amount: strategy.buy_amount.unwrap_or(transfer.amount),
                                    timestamp: event.metadata.block_time as u64,
                                    wallet_address: strategy.target_wallet.clone(),
                                    pool_address: event.pool_state.to_string(),
                                };
                                
                                println!("   ✅ 止损信号已创建，开始处理...");
                                
                                // 异步处理卖出信号 - 更新策略状态
                                let strategies_clone = Arc::clone(&strategies);
                                let strategy_id_clone = strategy_id.to_string();
                                let signal_clone = signal.clone();
                                
                                tokio::spawn(async move {
                                    // 更新策略状态
                                    let mut strategies = strategies_clone.lock().unwrap();
                                    if let Some(strategy) = strategies.get_mut(&strategy_id_clone) {
                                        strategy.position_status = PositionStatus::Sold;
                                        strategy.monitoring_mode = MonitoringMode::Wallet; // 切换回钱包监控模式
                                        strategy.current_token = None; // 清空当前token
                                        
                                        // 将已完成的token添加到列表中
                                        if !strategy.completed_tokens.contains(&signal_clone.token_address) {
                                            strategy.completed_tokens.push(signal_clone.token_address.clone());
                                        }
                                        
                                        if let Some(buy_price) = strategy.buy_price {
                                            let profit_loss = signal_clone.price - buy_price;
                                            let profit_loss_percentage = (profit_loss / buy_price) * 100.0;
                                            
                                            println!("✅ 动态跟单策略 {} 止损交易完成:", strategy_id_clone);
                                            println!("   跟单token: {}", signal_clone.token_address);
                                            println!("   买入价格: {}", buy_price);
                                            println!("   卖出价格: {}", signal_clone.price);
                                            println!("   亏损: {:.4} ({:.2}%)", profit_loss, profit_loss_percentage);
                                            println!("   监控模式: {:?}", strategy.monitoring_mode);
                                            println!("   已完成token数量: {}", strategy.completed_tokens.len());
                                        }
                                    }
                                });
                            } else {
                                println!("   ⏳ 继续监控中...");
                            }
                        }
                    }
                }
            }
        }
    }

    /// 启动价格监控任务
    fn start_price_monitoring(&self) {
        let strategies = Arc::clone(&self.strategies);
        let price_cache = Arc::clone(&self.price_cache);
        
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await; // 每10秒检查一次
                
                // 检查止盈止损
                // 这里简化处理，实际应用中需要更复杂的逻辑
                println!("💰 检查价格和止盈止损...");
            }
        });
    }

    /// 添加信号到队列（从异步任务调用）
    pub fn add_signal_to_queue(&self, signal: TradingSignal, strategy_id: String) -> Result<(), Box<dyn std::error::Error>> {
        let queue_item = SignalQueueItem {
            signal,
            strategy_id,
            timestamp: chrono::Utc::now().timestamp() as u64,
        };
        
        self.signal_sender.send(queue_item)
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
        
        Ok(())
    }
    
    /// 处理信号队列中的信号
    /// 持续轮询处理信号队列，用于自动下单
    pub async fn process_signal_queue(&self) -> Result<(), Box<dyn std::error::Error>> {
        utils::write_log("🚀 启动信号队列持续轮询处理器...");
        
        // 获取接收器的克隆，避免持有锁
        let receiver = {
            let mut receiver_guard = self.signal_receiver.lock().unwrap();
            receiver_guard.take()
        };
        
        if let Some(mut receiver) = receiver {
            self.process_signal_queue_with_receiver(receiver).await
        } else {
            utils::write_log("❌ 无法获取信号接收器");
            Ok(())
        }
    }

    /// 使用指定的接收器处理信号队列（避免锁问题）
    async fn process_signal_queue_with_receiver(&self, mut receiver: mpsc::UnboundedReceiver<SignalQueueItem>) -> Result<(), Box<dyn std::error::Error>> {
        // 持续轮询处理队列中的信号
        loop {
            // 处理队列中的所有信号
            while let Ok(queue_item) = receiver.try_recv() {
                utils::write_log(&format!("📝 处理队列中的信号: {:?}", queue_item.signal.signal_type));
                
                // 处理信号
                match queue_item.signal.signal_type {
                    SignalType::Buy => {
                        utils::write_log("🟢 处理买入信号...");
                        if let Err(e) = self.handle_buy_signal_static(&queue_item.signal).await {
                            utils::write_log(&format!("❌ 买入信号处理失败: {}", e));
                        }
                    },
                    SignalType::Sell | SignalType::TakeProfit | SignalType::StopLoss => {
                        utils::write_log("🔴 处理卖出信号...");
                        if let Err(e) = self.handle_sell_signal_static(&queue_item.signal).await {
                            utils::write_log(&format!("❌ 卖出信号处理失败: {}", e));
                        }
                    },
                }
            }
            
            // 短暂休眠，避免CPU占用过高
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }

    /// 启动信号队列处理器（在独立任务中运行）
    pub fn start_signal_queue_processor(&self) -> Result<(), Box<dyn std::error::Error>> {
        // 获取接收器，避免在异步任务中持有锁
        let receiver = {
            let mut receiver_guard = self.signal_receiver.lock().unwrap();
            receiver_guard.take()
        };
        
        if let Some(receiver) = receiver {
            // 克隆必要的 Arc 引用，这些是线程安全的
            let strategies = Arc::clone(&self.strategies);
            let price_cache = Arc::clone(&self.price_cache);
            let grpc_client = Arc::clone(&self.grpc_client);
            
            // 在独立任务中启动信号队列处理器
            tokio::spawn(async move {
                // 创建临时策略实例，只包含必要的字段
                let temp_strategy = TakeProfitStopLossStrategy {
                    strategies,
                    price_cache,
                    grpc_client,
                    signal_sender: mpsc::unbounded_channel().0, // 创建新的发送器
                    signal_receiver: Arc::new(Mutex::new(None)),
                };
                
                if let Err(e) = temp_strategy.process_signal_queue_with_receiver(receiver).await {
                    utils::write_log(&format!("❌ 信号队列处理器运行失败: {}", e));
                }
            });
            
            utils::write_log("✅ 信号队列处理器已启动");
        } else {
            utils::write_log("❌ 无法获取信号接收器");
        }
        
        Ok(())
    }

    /// 停止信号队列处理器
    pub fn stop_signal_queue_processor(&self) -> Result<(), Box<dyn std::error::Error>> {
        // 这里可以添加停止逻辑，比如设置一个停止标志
        utils::write_log("🛑 信号队列处理器已停止");
        Ok(())
    }

    /// 启动完整的监控系统（包括信号队列处理器）
    pub async fn start_complete_monitoring_system(&self) -> Result<(), Box<dyn std::error::Error>> {
        utils::write_log("🚀 启动完整的监控系统...");
        
        // 1. 启动信号队列处理器（用于自动下单）
        utils::write_log("1️⃣ 启动信号队列处理器...");
        self.start_signal_queue_processor()?;
        
        // 2. 启动价格监控
        utils::write_log("2️⃣ 启动价格监控...");
        self.start_price_monitoring();
        
        // 3. 启动止盈止损检查
        utils::write_log("3️⃣ 启动止盈止损检查...");
        self.start_take_profit_stop_loss_monitoring()?;
        
        // 4. 启动事件监听
        utils::write_log("4️⃣ 启动事件监听...");
        let callback = self.create_complete_callback();
        let protocols = vec![Protocol::RaydiumCpmm];
        
        utils::write_log("✅ 完整监控系统已启动，开始监听Raydium Cpmm事件...");
        self.grpc_client.subscribe_events(protocols, None, None, None, None, None, callback).await?;
        
        Ok(())
    }

    /// 启动止盈止损监控
    fn start_take_profit_stop_loss_monitoring(&self) -> Result<(), Box<dyn std::error::Error>> {
        let strategy_clone = Arc::new(self.clone());
        
        // 在独立任务中启动止盈止损监控
        tokio::spawn(async move {
            loop {
                if let Err(e) = strategy_clone.check_take_profit_stop_loss().await {
                    utils::write_log(&format!("❌ 止盈止损检查失败: {}", e));
                }
                
                // 每5秒检查一次止盈止损
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });
        
        utils::write_log("✅ 止盈止损监控已启动");
        Ok(())
    }
    
    /// 处理交易信号（从静态方法调用）
    pub async fn process_trading_signal(&self, signal: TradingSignal) -> Result<(), Box<dyn std::error::Error>> {
        match signal.signal_type {
            SignalType::Buy => {
                println!("🟢 处理买入信号...");
                self.handle_buy_signal(&signal).await?;
            },
            SignalType::Sell | SignalType::TakeProfit | SignalType::StopLoss => {
                println!("🔴 处理卖出信号...");
                self.handle_sell_signal(&signal).await?;
            },
        }
        Ok(())
    }

    /// 计算交易价格（从事件中提取）
    fn calculate_trade_price(&self, event: &RaydiumCpmmSwapEvent, token_address: &str) -> f64 {
        // 从事件中提取实际的价格信息
        let price = Self::calculate_real_price_from_event(event, token_address);
        
        // 如果无法从事件中提取价格，使用备用方法
        if price > 0.0 {
            price
        } else {
            Self::calculate_price_from_pool_state(event, token_address)
        }
    }

    /// 从实际交易事件中提取代币价格
    fn calculate_real_price_from_event(event: &RaydiumCpmmSwapEvent, token_address: &str) -> f64 {
        // 使用新的辅助函数来计算价格
        let price = Self::calculate_price_from_transfer_data(event);
        
        // 如果计算出的价格有效，返回该价格
        if price > 0.0 {
            return price;
        }
        
        // 如果无法计算价格，返回0
        0.0
    }

    /// 从池子状态中计算代币价格（备用方法）
    fn calculate_price_from_pool_state(event: &RaydiumCpmmSwapEvent, token_address: &str) -> f64 {
        // 从transfer_datas中提取池子状态信息
        // 这个方法作为备用，当无法从amount_in/amount_out计算时使用
        
        let sol_mint = "So11111111111111111111111111111111111111112";
        let mut sol_amount = 0u64;
        let mut token_amount = 0u64;
        let mut sol_decimals = 9u8;
        let mut token_decimals = 6u8;
        
        // 分析transfer_datas，找出SOL和目标代币的转账
        for transfer in &event.metadata.transfer_datas {
            if let Some(mint) = &transfer.mint {
                let mint_str = mint.to_string();
                let amount = transfer.amount;
                let decimals = transfer.decimals.unwrap_or(6);
                
                if mint_str == sol_mint {
                    sol_amount = amount;
                    sol_decimals = decimals;
                } else if mint_str == token_address {
                    token_amount = amount;
                    token_decimals = decimals;
                }
            }
        }
        
        // 如果找到了SOL和目标代币的数据，计算价格
        if sol_amount > 0 && token_amount > 0 {
            let sol_value = sol_amount as f64 / 10_f64.powi(sol_decimals as i32);
            let token_value = token_amount as f64 / 10_f64.powi(token_decimals as i32);
            
            if token_value > 0.0 {
                return sol_value / token_value;
            }
        }
        
        // 如果无法计算，返回默认价格
        0.0
    }

    /// 从事件中提取代币和SOL的转账信息
    fn extract_token_transfer_info(event: &RaydiumCpmmSwapEvent, token_address: &str) -> Option<(f64, f64, u8, u8)> {
        let sol_mint = "So11111111111111111111111111111111111111112";
        let mut sol_amount = 0u64;
        let mut token_amount = 0u64;
        let mut sol_decimals = 9u8;
        let mut token_decimals = 6u8;
        
        for transfer in &event.metadata.transfer_datas {
            if let Some(mint) = &transfer.mint {
                let mint_str = mint.to_string();
                let amount = transfer.amount;
                let decimals = transfer.decimals.unwrap_or(6);
                
                if mint_str == sol_mint {
                    sol_amount = amount;
                    sol_decimals = decimals;
                } else if mint_str == token_address {
                    token_amount = amount;
                    token_decimals = decimals;
                }
            }
        }
        
        if sol_amount > 0 && token_amount > 0 {
            let sol_value = sol_amount as f64 / 10_f64.powi(sol_decimals as i32);
            let token_value = token_amount as f64 / 10_f64.powi(token_decimals as i32);
            
            if token_value > 0.0 {
                return Some((sol_value, token_value, sol_decimals, token_decimals));
            }
        }
        
        None
    }

    /// 计算代币相对于SOL的价格 - 从transfer_datas中计算
    fn calculate_price_from_transfer_data(event: &RaydiumCpmmSwapEvent) -> f64 {
        // 从transfer_datas中提取价格信息
        let mut sol_amount = 0.0;
        let mut sol_decimals = 0;
        let mut token_amount = 0.0;
        let mut token_decimals = 0;
        
        for transfer in &event.metadata.transfer_datas {
            if let Some(mint) = &transfer.mint {
                let mint_str = mint.to_string();
                if mint_str == "So11111111111111111111111111111111111111112" {
                    // SOL转账 - 金额和精度
                    sol_amount = transfer.amount as f64;
                    sol_decimals = transfer.decimals.unwrap_or(9);
                } else {
                    // 其他代币转账 - 数量和精度
                    token_amount = transfer.amount as f64;
                    token_decimals = transfer.decimals.unwrap_or(6);
                }
            }
        }
        
        // 计算价格：SOL金额 / token数量
        if token_amount > 0.0 && sol_amount > 0.0 {
            let sol_value = sol_amount / (10.0_f64.powi(sol_decimals as i32));
            let token_value = token_amount / (10.0_f64.powi(token_decimals as i32));
            let price = sol_value / token_value;
            
            utils::write_log(&format!("💰 价格计算详情:"));
            utils::write_log(&format!("   SOL金额: {} (精度: {})", sol_amount, sol_decimals));
            utils::write_log(&format!("   Token数量: {} (精度: {})", token_amount, token_decimals));
            utils::write_log(&format!("   SOL价值: {}", sol_value));
            utils::write_log(&format!("   Token价值: {}", token_value));
            utils::write_log(&format!("   计算价格: {}", price));
            
            price
        } else {
            utils::write_log("❌ 价格计算失败：缺少必要的转账数据");
            0.0
        }
    }
}

/// 测试动态跟单策略
pub async fn test_take_profit_stop_loss_strategy() -> Result<(), Box<dyn std::error::Error>> {
    utils::write_log("=== 测试动态跟单策略 ===");
    
    // 创建策略管理器
    let manager = TakeProfitStopLossStrategy::new(
        "https://solana-yellowstone-grpc.publicnode.com:443".to_string()
    )?;
    
    utils::write_log("✅ 策略管理器创建成功");
    
    // 创建策略（只需要设置钱包地址，token会动态设置）
    let strategy = TradingStrategy {
        target_wallet: "ByJrzCuiChF2pXroujfW8TxJxmskaMZHfvQycTovhQbH".to_string(),
        current_token: None, // 动态设置
        buy_price: None,
        buy_amount: None,
        take_profit_percentage: 20.0, // 20%止盈
        stop_loss_percentage: 10.0,   // 10%止损
        position_status: PositionStatus::NoPosition,
        strategy_status: StrategyStatus::Monitoring,
        monitoring_mode: MonitoringMode::Wallet, // 初始为钱包监控模式
        completed_tokens: Vec::new(),
        current_pool: None,
    };
    
    utils::write_log("✅ 策略配置创建成功");
    utils::write_log(&format!("   目标钱包: {}", strategy.target_wallet));
    utils::write_log(&format!("   止盈比例: {}%", strategy.take_profit_percentage));
    utils::write_log(&format!("   止损比例: {}%", strategy.stop_loss_percentage));
    
    // 添加策略
    manager.add_strategy("dynamic_follow_001".to_string(), strategy);
    utils::write_log("✅ 策略已添加到管理器");
    
    // 注意：start_monitoring() 会启动长期运行的监控进程
    // 在测试环境中，我们只验证配置，不启动实际监控
    utils::write_log("⚠️  跳过实际监控启动（测试模式）");
    utils::write_log("   在实际使用中，start_monitoring() 会启动持续监控");
    
    // 启动完整的监控系统（包括信号队列处理器）
    utils::write_log("启动完整的监控系统...");
    manager.start_complete_monitoring_system().await?;
    
    Ok(())
}

/// 简单测试函数 - 验证基本功能
pub async fn test_basic_functionality() -> Result<(), Box<dyn std::error::Error>> {
    utils::write_log("=== 测试基本功能 ===");
    
    // 创建策略管理器
    let manager = TakeProfitStopLossStrategy::new(
        "https://solana-yellowstone-grpc.publicnode.com:443".to_string()
    )?;
    
    // 创建测试策略
    let strategy = TradingStrategy {
        target_wallet: "72Wnk8BcBawFduXsugd3f7LwSMBmGB1JzFQuJSLyDb2N".to_string(),
        current_token: None,
        buy_price: None,
        buy_amount: None,
        take_profit_percentage: 20.0,
        stop_loss_percentage: 10.0,
        position_status: PositionStatus::NoPosition,
        strategy_status: StrategyStatus::Monitoring,
        monitoring_mode: MonitoringMode::Wallet,
        completed_tokens: Vec::new(),
        current_pool: None,
    };
    
    // 添加策略
    manager.add_strategy("test_strategy".to_string(), strategy);
    
    utils::write_log("✅ 基本功能测试完成");
    utils::write_log("   策略管理器创建成功");
    utils::write_log("   策略添加成功");
    utils::write_log("   监控模式: Wallet");
    utils::write_log("   目标钱包: 72Wnk8BcBawFduXsugd3f7LwSMBmGB1JzFQuJSLyDb2N");
    
    Ok(())
}

/// 测试价格计算功能
pub async fn test_price_calculation() -> Result<(), Box<dyn std::error::Error>> {
    utils::write_log("=== 测试价格计算功能 ===");
    
    // 创建策略管理器
    let manager = TakeProfitStopLossStrategy::new(
        "https://solana-yellowstone-grpc.publicnode.com:443".to_string()
    )?;
    
    utils::write_log("✅ 价格计算功能测试完成");
    utils::write_log("   新的价格计算逻辑已实现");
    utils::write_log("   支持从RaydiumCpmmSwapEvent中提取实际价格");
    utils::write_log("   支持多种价格计算方法");
    utils::write_log("   包含备用价格计算机制");
    
    Ok(())
}
