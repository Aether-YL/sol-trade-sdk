use crate::{
    common::{AppConfig, Logger, log_info, log_warn, log_error, log_debug},
    strategy::{WalletMonitor, PriceMonitor, TakeProfitStopLossStrategy},
    SolanaTrade,
};
use anyhow::Result;
use solana_sdk::{pubkey::Pubkey, signature::Keypair};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::str::FromStr;
use anyhow;
use tokio::time::{Duration, interval};
use std::collections::HashMap;

/// 交易策略服务 - 核心服务类
/// 
/// 主要功能：
/// 1. 钱包监控：监听指定钱包的买入交易，触发跟单买入
/// 2. 持仓管理：跟踪买入的token，记录成本价和数量
/// 3. 价格监控：实时监控token价格变化
/// 4. 止盈止损：达到配置的止盈止损条件时自动卖出
/// 
/// 工作流程：
/// 1. 启动钱包监控，监听指定钱包的买入事件
/// 2. 检测到买入事件后，按照配置比例跟单买入
/// 3. 买入成功后，更新持仓信息并添加到价格监控
/// 4. 持续监控价格，达到止盈止损条件时自动卖出
/// 5. 循环执行上述流程
pub struct TradingStrategyService {
    config: Arc<AppConfig>,                    // 应用配置
    trade_client: Arc<SolanaTrade>,            // Solana交易客户端
    wallet_monitor: Arc<WalletMonitor>,        // 钱包监控器
    price_monitor: Arc<PriceMonitor>,          // 价格监控器
    strategy_executor: Arc<TakeProfitStopLossStrategy>, // 止盈止损策略执行器
    is_running: Arc<RwLock<bool>>,             // 服务运行状态
    positions: Arc<RwLock<HashMap<String, PositionInfo>>>, // 持仓信息管理
    monitoring_tasks: Arc<RwLock<Vec<tokio::task::JoinHandle<()>>>>, // 监控任务句柄
}

/// 持仓信息结构
/// 记录每个token的买入详情，用于计算盈亏和触发止盈止损
#[derive(Debug, Clone)]
pub struct PositionInfo {
    pub token_mint: String,                    // Token的Mint地址
    pub amount_token: u64,                     // 持有的Token数量
    pub buy_price: f64,                        // 买入时的价格
    pub buy_timestamp: chrono::DateTime<chrono::Utc>, // 买入时间
    pub total_cost_sol: u64,                   // 总成本（lamports）
    pub pool_state: Option<String>,            // 池状态信息
}

impl TradingStrategyService {
    /// 创建新的交易策略服务
    /// 
    /// 初始化流程：
    /// 1. 初始化日志系统
    /// 2. 验证配置文件
    /// 3. 创建私钥和交易客户端
    /// 4. 初始化各个策略模块
    pub async fn new(config: AppConfig) -> Result<Self> {
        // 初始化日志系统 - 统一输出到日志文件
        Logger::init(
            config.get_log_level(),
            if config.logging.log_to_file { Some(&config.logging.log_file_path) } else { None },
            true,
        )?;

        log_info!("初始化交易策略服务...");
        log_info!("配置摘要: {}", config.get_summary());

        // 验证配置 - 确保所有必要参数正确
        config.validate()?;
        log_info!("配置验证通过");

        // 创建私钥 - 从配置文件读取，确保安全性
        let private_key = bs58::decode(&config.authentication.private_key)
            .into_vec()
            .map_err(|e| anyhow::anyhow!("私钥解码失败: {}", e))?;
        
        let keypair = Keypair::from_bytes(&private_key)
            .map_err(|e| anyhow::anyhow!("私钥格式错误: {}", e))?;

        log_info!("私钥验证成功，钱包地址: {}", keypair.pubkey());

        // 创建交易客户端 - 用于执行实际的交易操作
        let trade_config = crate::common::TradeConfig {
            rpc_url: config.rpc.strategy_rpc_url.clone(),
            commitment: solana_sdk::commitment_config::CommitmentConfig::confirmed(),
            priority_fee: crate::common::PriorityFee {
                unit_limit: config.priority_fees.compute_unit_limit,
                unit_price: config.priority_fees.compute_unit_price,
                rpc_unit_limit: config.priority_fees.rpc_compute_unit_limit,
                rpc_unit_price: config.priority_fees.rpc_compute_unit_price,
                buy_tip_fee: config.priority_fees.buy_tip_fee,
                buy_tip_fees: vec![config.priority_fees.buy_tip_fee],
                smart_buy_tip_fee: 0.0,
                sell_tip_fee: config.priority_fees.sell_tip_fee,
            },
            swqos_configs: vec![],
            lookup_table_key: None,
        };

        let trade_client = Arc::new(SolanaTrade::new(Arc::new(keypair), trade_config).await);
        log_info!("交易客户端创建成功");

        let config_arc = Arc::new(config);

        // 创建各个策略模块 - 每个模块负责不同的功能
        let wallet_monitor = Arc::new(WalletMonitor::new(config_arc.clone(), trade_client.clone()));
        let price_monitor = Arc::new(PriceMonitor::new(config_arc.clone(), trade_client.clone()));
        let strategy_executor = Arc::new(TakeProfitStopLossStrategy::new(config_arc.clone(), trade_client.clone()));

        log_info!("交易策略服务初始化完成");

        Ok(Self {
            config: config_arc,
            trade_client,
            wallet_monitor,
            price_monitor,
            strategy_executor,
            is_running: Arc::new(RwLock::new(false)),
            positions: Arc::new(RwLock::new(HashMap::new())),
            monitoring_tasks: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// 启动服务 - 开始执行交易策略
    /// 
    /// 启动流程：
    /// 1. 初始化监控的钱包列表
    /// 2. 启动钱包监控任务
    /// 3. 启动价格监控任务
    /// 4. 启动完整的策略循环任务
    pub async fn start(&self) -> Result<()> {
        if self.is_running().await {
            log_warn!("服务已在运行中");
            return Ok(());
        }

        log_info!("启动交易策略服务...");

        // 初始化监控钱包
        self.initialize_monitored_wallets().await?;

        // 启动钱包监控任务
        let wallet_task = self.start_wallet_monitoring().await?;
        self.monitoring_tasks.write().await.push(wallet_task);

        // 启动价格监控任务
        let price_task = self.start_price_monitoring().await?;
        self.monitoring_tasks.write().await.push(price_task);

        // 启动完整的策略循环
        let strategy_task = self.start_complete_strategy_loop().await?;
        self.monitoring_tasks.write().await.push(strategy_task);

        // 启动DEX监听任务
        if self.config.wallet_monitoring.enable_dex_monitoring {
            self.wallet_monitor.start_dex_monitoring().await?;
            log_info!("DEX监听任务已启动");
        }

        // 设置服务状态为运行中
        *self.is_running.write().await = true;

        log_info!("交易策略服务启动完成");
        Ok(())
    }

    /// 停止交易策略服务
    /// 
    /// 功能说明：
    /// 1. 设置服务状态为停止
    /// 2. 停止DEX监听任务
    /// 3. 中止所有监控任务
    /// 4. 清空任务列表
    /// 5. 记录停止日志
    /// 
    /// 这是服务的优雅停止方法，确保所有任务都被正确清理
    pub async fn stop(&self) -> Result<()> {
        if !self.is_running().await {
            log_warn!("服务未在运行中");
            return Ok(());
        }

        log_info!("停止交易策略服务...");

        // 设置服务状态为停止
        *self.is_running.write().await = false;

        // 停止DEX监听任务
        if self.config.wallet_monitoring.enable_dex_monitoring {
            self.wallet_monitor.stop_dex_monitoring().await?;
            log_info!("DEX监听任务已停止");
        }

        // 中止所有监控任务
        let mut tasks = self.monitoring_tasks.write().await;
        for task in tasks.iter() {
            task.abort();
        }
        tasks.clear();

        log_info!("交易策略服务已停止");
        Ok(())
    }

    /// 检查服务是否运行中
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    /// 初始化监控钱包 - 从配置文件读取并添加要监控的钱包地址
    /// 
    /// 功能说明：
    /// 1. 检查钱包监控功能是否在配置中启用
    /// 2. 遍历配置文件中的钱包地址列表
    /// 3. 过滤掉占位符地址（如"YOUR_WALLET_ADDRESS_2"）
    /// 4. 将有效的钱包地址添加到监控列表
    /// 
    /// 这是钱包监控系统的初始化方法，确保服务启动时就有要监控的目标
    async fn initialize_monitored_wallets(&self) -> Result<()> {
        if !self.config.wallet_monitoring.enabled {
            log_info!("钱包监控已禁用");
            return Ok(());
        }

        log_info!("初始化监控钱包...");
        
        for wallet_address in &self.config.wallet_monitoring.wallet_addresses {
            if wallet_address != "YOUR_WALLET_ADDRESS_2" && !wallet_address.is_empty() {
                self.wallet_monitor.add_monitored_wallet(wallet_address.clone()).await?;
                log_info!("添加监控钱包: {}", wallet_address);
            }
        }

        Ok(())
    }

    /// 添加监控钱包
    pub async fn add_monitored_wallet(&self, wallet_address: String) -> Result<()> {
        self.wallet_monitor.add_monitored_wallet(wallet_address.clone()).await?;
        log_info!("手动添加监控钱包: {}", wallet_address);
        Ok(())
    }

    /// 移除监控钱包
    pub async fn remove_monitored_wallet(&self, wallet_address: &str) -> Result<()> {
        self.wallet_monitor.remove_monitored_wallet(wallet_address).await?;
        log_info!("移除监控钱包: {}", wallet_address);
        Ok(())
    }

    /// 获取监控的钱包列表
    pub async fn get_monitored_wallets(&self) -> Vec<String> {
        self.wallet_monitor.get_monitored_wallets().await
    }

    /// 获取持仓信息
    pub async fn get_positions(&self) -> Vec<PositionInfo> {
        let positions = self.positions.read().await;
        positions.values().cloned().collect()
    }

    /// 获取服务状态
    pub async fn get_service_status(&self) -> crate::common::ServiceStatus {
        crate::common::ServiceStatus {
            is_running: self.is_running().await,
            monitored_wallets_count: self.get_monitored_wallets().await.len(),
            positions_count: self.get_positions().await.len(),
            strategy_status: "Running".to_string(),
        }
    }

    /// 处理钱包买入事件 - 实现需求3.1：检测到买入时按配置比例跟单买入
    /// 
    /// 功能说明：
    /// 1. 接收钱包买入事件（来自钱包监控器）
    /// 2. 记录买入详情到日志
    /// 3. 转发给钱包监控器进行跟单处理
    /// 4. 按配置文件中的buy_ratio比例计算跟单金额
    /// 
    /// 这是跟单买入的入口点，实现了需求中的自动跟单逻辑
    pub async fn handle_wallet_buy_event(
        &self,
        wallet_address: &str,
        token_mint: &str,
        amount_sol: u64,
        amount_token: u64,
        signature: &str,
    ) -> Result<()> {
        log_info!(
            "处理钱包买入事件: 钱包={}, Token={}, 金额={} SOL, 数量={}",
            wallet_address,
            token_mint,
            amount_sol as f64 / 1_000_000_000.0,
            amount_token
        );

        // 转发给钱包监控器
        self.wallet_monitor.handle_buy_event(
            wallet_address,
            token_mint,
            amount_sol,
            amount_token,
            signature,
        ).await?;

        Ok(())
    }

    /// 更新Token价格 - 实现需求3.3：监听token价格变化
    /// 
    /// 功能说明：
    /// 1. 接收token的最新价格信息
    /// 2. 将价格信息转发给价格监控器
    /// 3. 价格监控器会自动检查止盈止损条件
    /// 4. 达到条件时自动触发卖出操作
    /// 
    /// 这是价格监控的入口点，实现了需求中的实时价格监控
    pub async fn update_token_price(
        &self,
        token_mint: &str,
        price: f64,
        pool_state: &str,
    ) -> Result<()> {
        log_debug!(
            "更新Token价格: Token={}, 价格={}, 池状态={}",
            token_mint,
            price,
            pool_state
        );

        // 转发给价格监控器
        self.price_monitor.update_price(token_mint, price, pool_state).await?;

        Ok(())
    }

    /// 添加持仓 - 实现需求3.2：买入成功后更新持仓成本
    /// 
    /// 功能说明：
    /// 1. 创建新的持仓记录，包含token信息、数量、买入价格、时间等
    /// 2. 将持仓信息保存到本地持仓管理
    /// 3. 同时添加到价格监控器，开始监控价格变化
    /// 4. 记录持仓添加日志，便于跟踪和调试
    /// 
    /// 这是持仓管理的核心方法，实现了需求中的持仓跟踪和成本更新
    pub async fn add_position(
        &self,
        token_mint: &str,
        amount_token: u64,
        buy_price: f64,
        total_cost_sol: u64,
        pool_state: Option<String>,
    ) -> Result<()> {
        let position = PositionInfo {
            token_mint: token_mint.to_string(),
            amount_token,
            buy_price,
            buy_timestamp: chrono::Utc::now(),
            total_cost_sol,
            pool_state,
        };

        let mut positions = self.positions.write().await;
        positions.insert(token_mint.to_string(), position.clone());

        // 同时添加到价格监控器
        self.price_monitor.add_position(token_mint, amount_token, buy_price, total_cost_sol).await?;

        log_info!(
            "添加持仓: Token={}, 数量={}, 买入价格={}, 成本={} SOL",
            token_mint,
            amount_token,
            buy_price,
            total_cost_sol as f64 / 1_000_000_000.0
        );

        Ok(())
    }

    /// 启动钱包监控 - 启动钱包监控后台任务
    /// 
    /// 功能说明：
    /// 1. 检查钱包监控是否在配置中启用
    /// 2. 创建后台任务，每30秒执行一次清理操作
    /// 3. 清理过期的钱包监控缓存数据
    /// 4. 返回任务句柄，用于后续的停止和清理
    /// 
    /// 这是钱包监控系统的启动方法，为跟单交易提供基础支持
    async fn start_wallet_monitoring(&self) -> Result<tokio::task::JoinHandle<()>> {
        if !self.config.wallet_monitoring.enabled {
            log_info!("钱包监控已禁用，跳过启动");
            return Ok(tokio::task::JoinHandle::noop());
        }

        log_info!("启动钱包监控...");

        let wallet_monitor = self.wallet_monitor.clone();
        let task = tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(30)); // 每30秒检查一次
            
            loop {
                interval.tick().await;
                
                // 清理过期缓存
                if let Err(e) = wallet_monitor.cleanup_expired_cache().await {
                    log_error!("清理钱包监控缓存失败: {}", e);
                }
            }
        });

        Ok(task)
    }

    /// 启动价格监控 - 启动价格监控后台任务
    /// 
    /// 功能说明：
    /// 1. 检查价格监控是否在配置中启用
    /// 2. 创建后台任务，每60秒执行一次清理操作
    /// 3. 清理过期的价格监控缓存数据
    /// 4. 返回任务句柄，用于后续的停止和清理
    /// 
    /// 这是价格监控系统的启动方法，为止盈止损策略提供基础支持
    async fn start_price_monitoring(&self) -> Result<tokio::task::JoinHandle<()>> {
        if !self.config.monitoring.enable_price_monitoring {
            log_info!("价格监控已禁用，跳过启动");
            return Ok(tokio::task::JoinHandle::noop());
        }

        log_info!("启动价格监控...");

        let price_monitor = self.price_monitor.clone();
        let task = tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(60)); // 每60秒检查一次
            
            loop {
                interval.tick().await;
                
                // 清理过期缓存
                if let Err(e) = price_monitor.cleanup_expired_cache().await {
                    log_error!("清理价格监控缓存失败: {}", e);
                }
            }
        });

        Ok(task)
    }

    /// 启动完整的策略循环 - 核心策略执行循环
    /// 
    /// 这个循环每10秒执行一次，包含以下步骤：
    /// 1. 检查钱包监控事件（买入/卖出）
    /// 2. 检查价格监控和止盈止损条件
    /// 3. 清理过期数据
    /// 4. 记录策略状态
    /// 
    /// 这是整个交易策略的核心循环，实现了需求中的3.1-3.3循环
    async fn start_complete_strategy_loop(&self) -> Result<tokio::task::JoinHandle<()>> {
        let config = self.config.clone();
        let wallet_monitor = self.wallet_monitor.clone();
        let price_monitor = self.price_monitor.clone();
        let strategy_executor = self.strategy_executor.clone();
        let positions = self.positions.clone();
        let is_running = self.is_running.clone();

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            
            while *is_running.read().await {
                interval.tick().await;
                
                if let Err(e) = Self::execute_strategy_cycle(
                    &config,
                    &wallet_monitor,
                    &price_monitor,
                    &strategy_executor,
                    &positions,
                ).await {
                    log_error!("策略循环执行失败: {}", e);
                }
            }
        });

        Ok(handle)
    }

    /// 执行策略循环 - 实现需求中的3.1-3.3循环逻辑
    /// 
    /// 策略循环步骤：
    /// 1. 检查钱包监控事件：监听指定钱包的买入交易，按配置比例跟单买入
    /// 2. 检查价格监控和止盈止损：监听token价格，达到止盈止损条件时卖出
    /// 3. 清理过期数据：维护系统性能
    /// 4. 记录策略状态：输出运行状态和统计信息
    /// 
    /// 这个方法是整个交易策略的核心，实现了完整的自动化交易流程
    async fn execute_strategy_cycle(
        config: &Arc<AppConfig>,
        wallet_monitor: &Arc<WalletMonitor>,
        price_monitor: &Arc<PriceMonitor>,
        strategy_executor: &Arc<TakeProfitStopLossStrategy>,
        positions: &Arc<RwLock<HashMap<String, PositionInfo>>>,
    ) -> Result<()> {
        // 1. 检查钱包监控事件
        Self::check_wallet_monitoring_events(wallet_monitor, price_monitor, positions).await?;
        
        // 2. 检查价格监控和止盈止损
        Self::check_price_monitoring_and_tp_sl(price_monitor, strategy_executor, positions).await?;
        
        // 3. 清理过期数据
        Self::cleanup_expired_data(wallet_monitor, price_monitor, strategy_executor).await?;
        
        // 4. 记录策略状态
        Self::log_strategy_status(config, wallet_monitor, price_monitor, strategy_executor, positions).await?;
        
        Ok(())
    }

    /// 检查钱包监控事件 - 实现需求3.1：监听指定wallet的交易
    /// 
    /// 功能说明：
    /// 1. 遍历所有监控的钱包地址
    /// 2. 检查每个钱包的最新交易记录
    /// 3. 检测到买入事件时，触发跟单买入（按配置比例）
    /// 4. 检测到卖出事件时，更新持仓信息
    /// 
    /// 这是跟单交易的核心逻辑，实现了需求中的钱包监控和跟单买入
    async fn check_wallet_monitoring_events(
        wallet_monitor: &Arc<WalletMonitor>,
        price_monitor: &Arc<PriceMonitor>,
        positions: &Arc<RwLock<HashMap<String, PositionInfo>>>,
    ) -> Result<()> {
        // 获取所有监控的钱包
        let monitored_wallets = wallet_monitor.get_monitored_wallets().await;
        
        for wallet_address in monitored_wallets {
            // 检查钱包的最新交易
            let transactions = wallet_monitor.get_wallet_transactions(&wallet_address).await;
            
            for transaction in transactions {
                match transaction.transaction_type {
                    crate::strategy::wallet_monitor::TransactionType::Buy => {
                        log_info!(
                            "检测到买入事件: 钱包={}, Token={}, 金额={} SOL",
                            wallet_address,
                            transaction.token_mint,
                            transaction.amount_sol as f64 / 1e9
                        );
                        
                        // 触发跟单买入
                        if let Err(e) = wallet_monitor.handle_buy_event(
                            &wallet_address,
                            &transaction.token_mint,
                            transaction.amount_sol,
                            transaction.amount_token,
                            &transaction.signature,
                        ).await {
                            log_error!("处理买入事件失败: {}", e);
                        }
                    }
                    crate::strategy::wallet_monitor::TransactionType::Sell => {
                        log_info!(
                            "检测到卖出事件: 钱包={}, Token={}, 金额={} Token",
                            wallet_address,
                            transaction.token_mint,
                            transaction.amount_token
                        );
                        
                        // 更新持仓信息
                        if let Err(e) = wallet_monitor.handle_sell_event(
                            &wallet_address,
                            &transaction.token_mint,
                            transaction.amount_sol,
                            transaction.amount_token,
                            &transaction.signature,
                        ).await {
                            log_error!("处理卖出事件失败: {}", e);
                        }
                    }
                }
            }
        }
        
        Ok(())
    }

    /// 检查价格监控和止盈止损 - 实现需求3.3：监听token价格，达到止盈止损时卖出
    /// 
    /// 功能说明：
    /// 1. 遍历所有持仓的token
    /// 2. 获取每个token的当前价格
    /// 3. 检查是否达到配置的止盈止损条件
    /// 4. 触发止盈止损卖出操作
    /// 
    /// 这是止盈止损策略的核心逻辑，实现了需求中的价格监控和自动卖出
    async fn check_price_monitoring_and_tp_sl(
        price_monitor: &Arc<PriceMonitor>,
        strategy_executor: &Arc<TakeProfitStopLossStrategy>,
        positions: &Arc<RwLock<HashMap<String, PositionInfo>>>,
    ) -> Result<()> {
        // 获取所有持仓
        let positions_guard = positions.read().await;
        let token_mints: Vec<String> = positions_guard.keys().cloned().collect();
        drop(positions_guard);
        
        for token_mint in token_mints {
            // 获取当前价格
            if let Some(current_price) = price_monitor.get_current_price(&token_mint).await {
                // 检查是否需要触发止盈止损
                if let Err(e) = price_monitor.check_take_profit_stop_loss(&token_mint, current_price).await {
                    log_error!("检查止盈止损失败: Token={}, 错误={}", token_mint, e);
                }
            }
        }
        
        Ok(())
    }

    /// 清理过期的数据 - 维护系统性能
    /// 
    /// 功能说明：
    /// 1. 清理钱包监控器的过期缓存
    /// 2. 清理价格监控器的过期缓存
    /// 3. 清理策略执行器的过期记录
    /// 4. 清理DEX交易记录和价格缓存
    /// 5. 记录清理结果
    /// 
    /// 这是系统维护方法，定期清理过期数据以保持性能
    async fn cleanup_expired_data(
        wallet_monitor: &Arc<WalletMonitor>,
        price_monitor: &Arc<PriceMonitor>,
        strategy_executor: &Arc<TakeProfitStopLossStrategy>,
    ) -> Result<()> {
        // 清理钱包监控器的过期缓存
        wallet_monitor.cleanup_expired_cache().await?;
        wallet_monitor.cleanup_completed_copy_trades().await?;
        
        // 清理DEX相关的过期数据
        wallet_monitor.cleanup_expired_price_cache().await?;
        wallet_monitor.cleanup_expired_dex_transactions().await?;

        // 清理价格监控器的过期缓存
        price_monitor.cleanup_expired_cache().await?;

        // 清理策略执行器的过期记录
        strategy_executor.cleanup_expired_records().await?;

        log_debug!("过期数据清理完成");
        Ok(())
    }

    /// 记录策略状态日志 - 提供系统运行状态的全面视图
    /// 
    /// 功能说明：
    /// 1. 记录监控钱包数量
    /// 2. 记录持仓数量
    /// 3. 记录价格监控状态
    /// 4. 记录策略执行状态
    /// 5. 记录DEX监听状态
    /// 6. 记录系统整体运行状态
    /// 
    /// 这是系统状态监控方法，定期输出关键指标
    async fn log_strategy_status(
        config: &Arc<AppConfig>,
        wallet_monitor: &Arc<WalletMonitor>,
        price_monitor: &Arc<PriceMonitor>,
        strategy_executor: &Arc<TakeProfitStopLossStrategy>,
        positions: &Arc<RwLock<HashMap<String, PositionInfo>>>,
    ) -> Result<()> {
        let monitored_wallets = wallet_monitor.get_monitored_wallets().await;
        let positions = positions.read().await;
        let price_summary = price_monitor.get_positions_summary().await;
        let strategy_stats = strategy_executor.get_statistics().await;

        // 获取DEX监听状态
        let dex_status = if config.wallet_monitoring.enable_dex_monitoring {
            let dex_transactions = wallet_monitor.get_dex_transactions().await;
            let token_prices = wallet_monitor.get_all_token_prices().await;
            format!("启用 (交易记录: {}, 价格缓存: {})", dex_transactions.len(), token_prices.len())
        } else {
            "禁用".to_string()
        };

        log_info!(
            "策略状态 - 监控钱包: {}, 持仓: {}, 价格监控: {}, 策略执行: {}, DEX监听: {}",
            monitored_wallets.len(),
            positions.len(),
            if config.monitoring.enable_price_monitoring { "启用" } else { "禁用" },
            if config.strategy.enable_take_profit_stop_loss { "启用" } else { "禁用" },
            dex_status
        );

        // 记录持仓摘要
        if !positions.is_empty() {
            let total_value = positions.values().map(|p| p.total_cost_sol).sum::<u64>();
            log_info!(
                "持仓摘要 - 总持仓: {}, 总价值: {} SOL",
                positions.len(),
                total_value as f64 / 1_000_000_000.0
            );
        }

        // 记录价格监控摘要
        if config.monitoring.enable_price_monitoring {
            log_info!(
                "价格监控摘要 - 监控持仓: {}, 总成本: {} SOL, 当前价值: {} SOL",
                price_summary.total_positions,
                price_summary.total_cost_sol as f64 / 1_000_000_000.0,
                price_summary.current_value_sol as f64 / 1_000_000_000.0
            );
        }

        // 记录策略执行摘要
        if config.strategy.enable_take_profit_stop_loss {
            log_info!(
                "策略执行摘要 - 总执行: {}, 成功: {}, 失败: {}",
                strategy_stats.total_executions,
                strategy_stats.successful_executions,
                strategy_stats.failed_executions
            );
        }

        Ok(())
    }

    /// 获取策略统计信息 - 提供完整的系统运行数据
    /// 
    /// 功能说明：
    /// 1. 获取服务状态信息
    /// 2. 获取监控钱包列表
    /// 3. 获取持仓信息
    /// 4. 获取价格监控摘要
    /// 5. 获取策略执行统计
    /// 6. 获取DEX监听统计
    /// 
    /// 这是系统统计信息的汇总方法，用于监控和分析
    pub async fn get_strategy_statistics(&self) -> StrategyStatistics {
        let service_status = self.get_service_status().await;
        let monitored_wallets = self.get_monitored_wallets().await;
        let positions = self.get_positions().await;
        let positions_summary = self.price_monitor.get_positions_summary().await;
        let strategy_stats = self.strategy_executor.get_statistics().await;

        // 获取DEX监听统计
        let dex_transactions = self.wallet_monitor.get_dex_transactions().await;
        let token_prices = self.wallet_monitor.get_all_token_prices().await;

        log_info!(
            "策略统计 - DEX交易: {}, 价格缓存: {}, 监控钱包: {}, 持仓: {}",
            dex_transactions.len(),
            token_prices.len(),
            monitored_wallets.len(),
            positions.len()
        );

        StrategyStatistics {
            service_status,
            monitored_wallets,
            positions,
            positions_summary,
            strategy_stats,
        }
    }

    /// 模拟钱包买入事件（用于测试） - 模拟钱包买入事件，便于测试和调试
    /// 
    /// 功能说明：
    /// 1. 创建模拟的钱包买入事件
    /// 2. 调用实际的事件处理方法
    /// 3. 生成唯一的模拟签名
    /// 4. 记录模拟事件日志
    /// 
    /// 这个方法主要用于开发和测试阶段，验证跟单买入逻辑的正确性
    pub async fn simulate_wallet_buy_event(
        &self,
        wallet_address: &str,
        token_mint: &str,
        amount_sol: u64,
        amount_token: u64,
    ) -> Result<()> {
        log_info!("模拟钱包买入事件: 钱包={}, Token={}", wallet_address, token_mint);
        
        self.handle_wallet_buy_event(
            wallet_address,
            token_mint,
            amount_sol,
            amount_token,
            &format!("sim_{}", chrono::Utc::now().timestamp_millis()),
        ).await?;

        Ok(())
    }

    /// 模拟价格更新（用于测试） - 模拟token价格变化，便于测试止盈止损逻辑
    /// 
    /// 功能说明：
    /// 1. 创建模拟的价格更新事件
    /// 2. 调用实际的价格更新方法
    /// 3. 记录模拟价格更新日志
    /// 4. 触发止盈止损检查逻辑
    /// 
    /// 这个方法主要用于开发和测试阶段，验证价格监控和止盈止损逻辑的正确性
    pub async fn simulate_price_update(
        &self,
        token_mint: &str,
        price: f64,
        pool_state: &str,
    ) -> Result<()> {
        log_info!("模拟价格更新: Token={}, 价格={}", token_mint, price);
        
        self.update_token_price(token_mint, price, pool_state).await?;

        Ok(())
    }

    /// 手动触发止盈止损失查 - 手动触发指定token的止盈止损检查
    /// 
    /// 功能说明：
    /// 1. 获取指定token的当前价格
    /// 2. 手动更新价格以触发止盈止损检查
    /// 3. 记录手动触发日志
    /// 4. 如果无法获取价格，记录警告日志
    /// 
    /// 这个方法主要用于手动干预和调试，可以立即检查某个token的止盈止损状态
    pub async fn trigger_tp_sl_check(&self, token_mint: &str) -> Result<()> {
        log_info!("手动触发止盈止损检查: Token={}", token_mint);
        
        // 获取当前价格
        if let Some(current_price) = self.price_monitor.get_current_price(token_mint).await {
            // 更新价格以触发检查
            self.update_token_price(token_mint, current_price, "").await?;
            log_info!("止盈止损检查完成: Token={}, 当前价格={}", token_mint, current_price);
        } else {
            log_warn!("无法获取Token价格: {}", token_mint);
        }
        
        Ok(())
    }

    /// 获取DEX交易记录
    pub async fn get_dex_transactions(&self) -> Vec<crate::strategy::wallet_monitor::DexTransaction> {
        self.wallet_monitor.get_dex_transactions().await
    }

    /// 获取特定DEX的交易记录
    pub async fn get_dex_transactions_by_type(&self, dex_type: &crate::trading::factory::DexType) -> Vec<crate::strategy::wallet_monitor::DexTransaction> {
        self.wallet_monitor.get_dex_transactions_by_type(dex_type).await
    }

    /// 获取特定Token的DEX交易记录
    pub async fn get_dex_transactions_by_token(&self, token_mint: &str) -> Vec<crate::strategy::wallet_monitor::DexTransaction> {
        self.wallet_monitor.get_dex_transactions_by_token(token_mint).await
    }

    /// 获取Token的当前价格
    pub async fn get_token_price(&self, token_mint: &str) -> Option<crate::strategy::wallet_monitor::TokenPrice> {
        self.wallet_monitor.get_token_price(token_mint).await
    }

    /// 获取所有Token价格
    pub async fn get_all_token_prices(&self) -> Vec<crate::strategy::wallet_monitor::TokenPrice> {
        self.wallet_monitor.get_all_token_prices().await
    }
}

/// 策略统计信息 - 包含策略运行的完整统计数据和状态信息
/// 
/// 这个结构体提供了以下信息：
/// - 服务运行状态：是否运行、监控钱包数量、持仓数量等
/// - 监控钱包列表：当前正在监控的钱包地址
/// - 持仓信息：所有持仓的详细信息
/// - 持仓摘要：总成本、当前价值、盈亏等汇总信息
/// - 策略统计：执行次数、成功率等策略相关统计
/// 
/// 用于监控策略运行状态、分析性能和调试问题
#[derive(Debug, Clone)]
pub struct StrategyStatistics {
    pub service_status: crate::common::ServiceStatus,
    pub monitored_wallets: Vec<String>,
    pub positions: Vec<PositionInfo>,
    pub positions_summary: crate::strategy::price_monitor::PositionsSummary,
    pub strategy_stats: crate::strategy::take_profit_stop_loss::StrategyStats,
}
