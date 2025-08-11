use crate::{
    common::{AppConfig, log_info, log_warn, log_error, log_debug},
    trading::factory::DexType,
    SolanaTrade,
};
use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use std::str::FromStr;

/// DEX交易记录
#[derive(Debug, Clone)]
pub struct DexTransaction {
    pub dex_type: DexType,
    pub token_mint: String,
    pub transaction_type: TransactionType,
    pub amount_sol: u64,
    pub amount_token: u64,
    pub timestamp: DateTime<Utc>,
    pub signature: String,
    pub pool_address: Option<String>,
    pub price_sol_per_token: f64,
}

/// 价格信息
#[derive(Debug, Clone)]
pub struct TokenPrice {
    pub token_mint: String,
    pub price_sol: f64,
    pub price_usd: Option<f64>,
    pub volume_24h: Option<u64>,
    pub timestamp: DateTime<Utc>,
    pub source: String, // DEX名称
}

/// 价格缓存
#[derive(Debug, Clone)]
pub struct PriceCache {
    pub prices: HashMap<String, TokenPrice>,
    pub last_update: DateTime<Utc>,
}

/// 钱包交易记录
#[derive(Debug, Clone)]
pub struct WalletTransaction {
    pub wallet_address: String,
    pub token_mint: String,
    pub transaction_type: TransactionType,
    pub amount_sol: u64,
    pub amount_token: u64,
    pub timestamp: DateTime<Utc>,
    pub signature: String,
}

/// 交易类型
#[derive(Debug, Clone)]
pub enum TransactionType {
    Buy,
    Sell,
}

/// 跟单交易记录
#[derive(Debug, Clone)]
pub struct CopyTradeRecord {
    pub original_transaction: WalletTransaction,
    pub copy_amount_sol: u64,
    pub copy_amount_token: u64,
    pub copy_timestamp: DateTime<Utc>,
    pub copy_signature: Option<String>,
    pub status: CopyTradeStatus,
}

/// 跟单交易状态
#[derive(Debug, Clone)]
pub enum CopyTradeStatus {
    Pending,
    Executing,
    Completed,
    Failed(String),
}

/// 钱包监控器
pub struct WalletMonitor {
    config: Arc<AppConfig>,
    trade_client: Arc<SolanaTrade>,
    monitored_wallets: Arc<RwLock<HashMap<String, Vec<WalletTransaction>>>>,
    transaction_cache: Arc<RwLock<HashMap<String, DateTime<Utc>>>>,
    copy_trades: Arc<RwLock<Vec<CopyTradeRecord>>>,
    /// DEX交易记录
    dex_transactions: Arc<RwLock<Vec<DexTransaction>>>,
    /// 价格缓存
    price_cache: Arc<RwLock<PriceCache>>,
    /// DEX监听任务句柄
    dex_monitoring_task: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl WalletMonitor {
    /// 创建新的钱包监控器
    pub fn new(config: Arc<AppConfig>, trade_client: Arc<SolanaTrade>) -> Self {
        Self {
            config,
            trade_client,
            monitored_wallets: Arc::new(RwLock::new(HashMap::new())),
            transaction_cache: Arc::new(RwLock::new(HashMap::new())),
            copy_trades: Arc::new(RwLock::new(Vec::new())),
            dex_transactions: Arc::new(RwLock::new(Vec::new())),
            price_cache: Arc::new(RwLock::new(PriceCache {
                prices: HashMap::new(),
                last_update: Utc::now(),
            })),
            dex_monitoring_task: Arc::new(RwLock::new(None)),
        }
    }

    /// 添加监控钱包
    pub async fn add_monitored_wallet(&self, wallet_address: String) -> Result<()> {
        let mut wallets = self.monitored_wallets.write().await;
        if !wallets.contains_key(&wallet_address) {
            wallets.insert(wallet_address.clone(), Vec::new());
            log_info!("添加监控钱包: {}", wallet_address);
        }
        Ok(())
    }

    /// 移除监控钱包
    pub async fn remove_monitored_wallet(&self, wallet_address: &str) -> Result<()> {
        let mut wallets = self.monitored_wallets.write().await;
        if wallets.remove(wallet_address).is_some() {
            log_info!("移除监控钱包: {}", wallet_address);
        }
        Ok(())
    }

    /// 获取监控的钱包列表
    pub async fn get_monitored_wallets(&self) -> Vec<String> {
        let wallets = self.monitored_wallets.read().await;
        wallets.keys().cloned().collect()
    }

    /// 处理买入事件
    pub async fn handle_buy_event(
        &self,
        wallet_address: &str,
        token_mint: &str,
        amount_sol: u64,
        amount_token: u64,
        signature: &str,
    ) -> Result<()> {
        // 检查是否在监控列表中
        let wallets = self.monitored_wallets.read().await;
        if !wallets.contains_key(wallet_address) {
            return Ok(());
        }

        // 检查买入金额是否达到阈值
        if amount_sol < self.config.wallet_monitoring.min_buy_amount_sol {
            log_debug!(
                "买入金额 {} 低于阈值 {}，跳过监控",
                amount_sol as f64 / 1_000_000_000.0,
                self.config.wallet_monitoring.min_buy_amount_sol as f64 / 1_000_000_000.0
            );
            return Ok(());
        }

        // 检查是否重复处理
        let cache_key = format!("{}_{}", wallet_address, signature);
        let mut cache = self.transaction_cache.write().await;
        if cache.contains_key(&cache_key) {
            return Ok(());
        }

        // 记录交易
        let transaction = WalletTransaction {
            wallet_address: wallet_address.to_string(),
            token_mint: token_mint.to_string(),
            transaction_type: TransactionType::Buy,
            amount_sol,
            amount_token,
            timestamp: Utc::now(),
            signature: signature.to_string(),
        };

        // 添加到监控记录
        drop(wallets); // 释放读锁
        let mut wallets = self.monitored_wallets.write().await;
        if let Some(transactions) = wallets.get_mut(wallet_address) {
            transactions.push(transaction.clone());
            log_info!(
                "监控到买入事件: 钱包={}, Token={}, 金额={} SOL, 数量={}",
                wallet_address,
                token_mint,
                amount_sol as f64 / 1_000_000_000.0,
                amount_token
            );
        }

        // 添加到缓存
        cache.insert(cache_key, Utc::now());

        // 触发跟单交易
        self.trigger_copy_trade(&transaction).await?;

        Ok(())
    }

    /// 处理卖出事件
    pub async fn handle_sell_event(
        &self,
        wallet_address: &str,
        token_mint: &str,
        amount_sol: u64,
        amount_token: u64,
        signature: &str,
    ) -> Result<()> {
        // 检查是否在监控列表中
        let wallets = self.monitored_wallets.read().await;
        if !wallets.contains_key(wallet_address) {
            return Ok(());
        }

        // 检查是否重复处理
        let cache_key = format!("{}_{}", wallet_address, signature);
        let mut cache = self.transaction_cache.write().await;
        if cache.contains_key(&cache_key) {
            return Ok(());
        }

        // 记录交易
        let transaction = WalletTransaction {
            wallet_address: wallet_address.to_string(),
            token_mint: token_mint.to_string(),
            transaction_type: TransactionType::Sell,
            amount_sol,
            amount_token,
            timestamp: Utc::now(),
            signature: signature.to_string(),
        };

        // 添加到监控记录
        drop(wallets); // 释放读锁
        let mut wallets = self.monitored_wallets.write().await;
        if let Some(transactions) = wallets.get_mut(wallet_address) {
            transactions.push(transaction.clone());
            log_info!(
                "监控到卖出事件: 钱包={}, Token={}, 金额={} SOL, 数量={}",
                wallet_address,
                token_mint,
                amount_sol as f64 / 1_000_000_000.0,
                amount_token
            );
        }

        // 添加到缓存
        cache.insert(cache_key, Utc::now());

        Ok(())
    }

    /// 触发跟单交易
    async fn trigger_copy_trade(&self, transaction: &WalletTransaction) -> Result<()> {
        if !self.config.strategy.enable_copy_trading {
            return Ok(());
        }

        // 计算跟单买入金额
        let copy_amount = (transaction.amount_sol as f64 * self.config.take_profit_stop_loss.buy_ratio) as u64;
        
        // 检查买入金额限制
        if copy_amount < self.config.take_profit_stop_loss.min_buy_amount_sol {
            log_warn!("跟单金额 {} 低于最小限制 {}", copy_amount, self.config.take_profit_stop_loss.min_buy_amount_sol);
            return Ok(());
        }

        if copy_amount > self.config.take_profit_stop_loss.max_buy_amount_sol {
            log_warn!("跟单金额 {} 超过最大限制 {}", copy_amount, self.config.take_profit_stop_loss.max_buy_amount_sol);
            return Ok(());
        }

        // 创建跟单记录
        let copy_trade = CopyTradeRecord {
            original_transaction: transaction.clone(),
            copy_amount_sol: copy_amount,
            copy_amount_token: 0, // 将在执行成功后更新
            copy_timestamp: Utc::now(),
            copy_signature: None,
            status: CopyTradeStatus::Pending,
        };

        // 添加到跟单记录
        {
            let mut copy_trades = self.copy_trades.write().await;
            copy_trades.push(copy_trade.clone());
        }

        log_info!(
            "触发跟单交易: Token={}, 金额={} SOL (原始交易: {} SOL)",
            transaction.token_mint,
            copy_amount as f64 / 1_000_000_000.0,
            transaction.amount_sol as f64 / 1_000_000_000.0
        );

        // 执行跟单买入交易
        match self.execute_copy_trade(&transaction.token_mint, copy_amount, &copy_trade).await {
            Ok(_) => {
                log_info!("跟单交易执行成功: Token={}", transaction.token_mint);
            }
            Err(e) => {
                log_error!("跟单交易执行失败: Token={}, 错误: {}", transaction.token_mint, e);
                // 更新跟单状态为失败
                self.update_copy_trade_status(&copy_trade, CopyTradeStatus::Failed(e.to_string())).await?;
            }
        }

        Ok(())
    }

    /// 执行跟单交易
    async fn execute_copy_trade(&self, token_mint: &str, amount_sol: u64, copy_trade: &CopyTradeRecord) -> Result<()> {
        let mint_pubkey = match Pubkey::from_str(token_mint) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("无效的Token地址: {}, 错误: {}", token_mint, e)),
        };

        // 更新跟单状态为执行中
        self.update_copy_trade_status(copy_trade, CopyTradeStatus::Executing).await?;

        let recent_blockhash = self.trade_client.rpc.get_latest_blockhash().await?;
        let slippage_basis_points = Some(self.config.trading.default_slippage_basis_points);

        // 使用Raydium CPMM执行买入
        let raydium_params = crate::trading::core::params::RaydiumCpmmParams {
            pool_state: None, // 自动计算
            mint_token_program: Some(spl_token::ID),
            mint_token_in_pool_state_index: Some(1),
            minimum_amount_out: None,
            auto_handle_wsol: self.config.trading.auto_handle_wsol,
        };

        // 执行买入交易
        self.trade_client.buy(
            crate::trading::factory::DexType::RaydiumCpmm,
            mint_pubkey,
            None,
            amount_sol,
            slippage_basis_points,
            recent_blockhash,
            None,
            Some(Box::new(raydium_params)),
        ).await?;

        // 更新跟单状态为完成
        self.update_copy_trade_status(copy_trade, CopyTradeStatus::Completed).await?;

        // 计算买入的Token数量（估算）
        let estimated_token_amount = self.estimate_token_amount(amount_sol, token_mint).await?;
        
        // 更新跟单记录中的Token数量
        self.update_copy_trade_token_amount(copy_trade, estimated_token_amount).await?;

        // 通知价格监控器添加持仓
        self.notify_position_added(token_mint, estimated_token_amount, amount_sol).await?;

        Ok(())
    }

    /// 估算买入的Token数量
    async fn estimate_token_amount(&self, amount_sol: u64, token_mint: &str) -> Result<u64> {
        // 这里可以根据当前池状态计算Token数量
        // 简化实现：使用固定比例估算
        let estimated_ratio = 1000.0; // 假设1 SOL可以买1000个Token
        let estimated_amount = (amount_sol as f64 * estimated_ratio) as u64;
        
        log_debug!(
            "估算Token数量: {} SOL -> {} Token (比例: {})",
            amount_sol as f64 / 1_000_000_000.0,
            estimated_amount,
            estimated_ratio
        );
        
        Ok(estimated_amount)
    }

    /// 通知添加持仓
    async fn notify_position_added(&self, token_mint: &str, amount_token: u64, total_cost_sol: u64) -> Result<()> {
        // 这里应该调用价格监控器的add_position方法
        // 由于模块间依赖关系，我们通过日志记录
        log_info!(
            "跟单买入成功，添加持仓: Token={}, 数量={}, 成本={} SOL",
            token_mint,
            amount_token,
            total_cost_sol as f64 / 1_000_000_000.0
        );
        
        // TODO: 集成价格监控器，添加持仓记录
        // self.price_monitor.add_position(token_mint, amount_token, 0.0, total_cost_sol).await?;
        
        Ok(())
    }

    /// 更新跟单交易状态
    async fn update_copy_trade_status(&self, copy_trade: &CopyTradeRecord, status: CopyTradeStatus) -> Result<()> {
        let mut copy_trades = self.copy_trades.write().await;
        if let Some(record) = copy_trades.iter_mut().find(|r| 
            r.original_transaction.signature == copy_trade.original_transaction.signature &&
            r.copy_timestamp == copy_trade.copy_timestamp
        ) {
            record.status = status;
            log_debug!("更新跟单状态: {:?}", status);
        }
        Ok(())
    }

    /// 更新跟单交易的Token数量
    async fn update_copy_trade_token_amount(&self, copy_trade: &CopyTradeRecord, amount_token: u64) -> Result<()> {
        let mut copy_trades = self.copy_trades.write().await;
        if let Some(record) = copy_trades.iter_mut().find(|r| 
            r.original_transaction.signature == copy_trade.original_transaction.signature &&
            r.copy_timestamp == copy_trade.copy_timestamp
        ) {
            record.copy_amount_token = amount_token;
            log_debug!("更新跟单Token数量: {}", amount_token);
        }
        Ok(())
    }

    /// 获取钱包的交易历史
    pub async fn get_wallet_transactions(&self, wallet_address: &str) -> Vec<WalletTransaction> {
        let wallets = self.monitored_wallets.read().await;
        wallets.get(wallet_address).cloned().unwrap_or_default()
    }

    /// 获取跟单交易记录
    pub async fn get_copy_trades(&self) -> Vec<CopyTradeRecord> {
        let copy_trades = self.copy_trades.read().await;
        copy_trades.clone()
    }

    /// 获取Token的当前价格
    pub async fn get_token_price(&self, token_mint: &str) -> Option<TokenPrice> {
        let price_cache = self.price_cache.read().await;
        price_cache.prices.get(token_mint).cloned()
    }

    /// 获取所有Token价格
    pub async fn get_all_token_prices(&self) -> Vec<TokenPrice> {
        let price_cache = self.price_cache.read().await;
        price_cache.prices.values().cloned().collect()
    }

    /// 获取DEX交易记录
    pub async fn get_dex_transactions(&self) -> Vec<DexTransaction> {
        let dex_transactions = self.dex_transactions.read().await;
        dex_transactions.clone()
    }

    /// 获取特定DEX的交易记录
    pub async fn get_dex_transactions_by_type(&self, dex_type: &DexType) -> Vec<DexTransaction> {
        let dex_transactions = self.dex_transactions.read().await;
        dex_transactions
            .iter()
            .filter(|tx| tx.dex_type == *dex_type)
            .cloned()
            .collect()
    }

    /// 获取特定Token的DEX交易记录
    pub async fn get_dex_transactions_by_token(&self, token_mint: &str) -> Vec<DexTransaction> {
        let dex_transactions = self.dex_transactions.read().await;
        dex_transactions
            .iter()
            .filter(|tx| tx.token_mint == token_mint)
            .cloned()
            .collect()
    }

    /// 清理过期的缓存
    pub async fn cleanup_expired_cache(&self) -> Result<()> {
        let now = Utc::now();
        let mut cache = self.transaction_cache.write().await;
        
        // 清理24小时前的缓存
        let expired_keys: Vec<String> = cache
            .iter()
            .filter(|(_, &timestamp)| {
                now.signed_duration_since(timestamp).num_hours() > 24
            })
            .map(|(key, _)| key.clone())
            .collect();



        for key in expired_keys {
            cache.remove(&key);
        }

        if !expired_keys.is_empty() {
            log_debug!("清理了 {} 个过期缓存", expired_keys.len());
        }

        Ok(())
    }

    /// 清理过期的价格缓存
    pub async fn cleanup_expired_price_cache(&self) -> Result<()> {
        let now = Utc::now();
        let mut price_cache = self.price_cache.write().await;
        let ttl_seconds = self.config.wallet_monitoring.price_cache_ttl_seconds;
        
        let expired_tokens: Vec<String> = price_cache
            .prices
            .iter()
            .filter(|(_, price)| {
                now.signed_duration_since(price.timestamp).num_seconds() > ttl_seconds as i64
            })
            .map(|(token_mint, _)| token_mint.clone())
            .collect();

        for token_mint in expired_tokens {
            price_cache.prices.remove(&token_mint);
        }

        if !expired_tokens.is_empty() {
            log_debug!("清理了 {} 个过期的价格缓存", expired_tokens.len());
        }

        Ok(())
    }

    /// 清理过期的DEX交易记录
    pub async fn cleanup_expired_dex_transactions(&self) -> Result<()> {
        let now = Utc::now();
        let mut dex_transactions = self.dex_transactions.write().await;
        
        // 清理24小时前的交易记录
        let before_count = dex_transactions.len();
        dex_transactions.retain(|tx| {
            now.signed_duration_since(tx.timestamp).num_hours() <= 24
        });

        let after_count = dex_transactions.len();
        let cleaned_count = before_count - after_count;

        if cleaned_count > 0 {
            log_debug!("清理了 {} 条过期的DEX交易记录", cleaned_count);
        }

        Ok(())
    }

    /// 清理完成的跟单记录
    pub async fn cleanup_completed_copy_trades(&self) -> Result<()> {
        let mut copy_trades = self.copy_trades.write().await;
        let before_count = copy_trades.len();
        
        copy_trades.retain(|trade| {
            !matches!(trade.status, CopyTradeStatus::Completed | CopyTradeStatus::Failed(_))
        });

        let after_count = copy_trades.len();
        let cleaned_count = before_count - after_count;

        if cleaned_count > 0 {
            log_info!("清理了 {} 条已完成的跟单记录", cleaned_count);
        }

        Ok(())
    }

    /// 启动DEX监听
    pub async fn start_dex_monitoring(&self) -> Result<()> {
        if !self.config.wallet_monitoring.enable_dex_monitoring {
            log_info!("DEX监听未启用，跳过启动");
            return Ok(());
        }

        // 检查是否已经在运行
        let mut task_handle = self.dex_monitoring_task.write().await;
        if task_handle.is_some() {
            log_warn!("DEX监听任务已在运行");
            return Ok(());
        }

        let config = self.config.clone();
        let dex_transactions = self.dex_transactions.clone();
        let price_cache = self.price_cache.clone();
        let trade_client = self.trade_client.clone();

        // 启动DEX监听任务
        let handle = tokio::spawn(async move {
            Self::run_dex_monitoring_loop(
                config,
                dex_transactions,
                price_cache,
                trade_client,
            ).await;
        });

        *task_handle = Some(handle);
        log_info!("DEX监听任务已启动");

        Ok(())
    }

    /// 停止DEX监听
    pub async fn stop_dex_monitoring(&self) -> Result<()> {
        let mut task_handle = self.dex_monitoring_task.write().await;
        if let Some(handle) = task_handle.take() {
            handle.abort();
            log_info!("DEX监听任务已停止");
        }
        Ok(())
    }

    /// DEX监听主循环
    async fn run_dex_monitoring_loop(
        config: Arc<AppConfig>,
        dex_transactions: Arc<RwLock<Vec<DexTransaction>>>,
        price_cache: Arc<RwLock<PriceCache>>,
        trade_client: Arc<SolanaTrade>,
    ) {
        let interval = tokio::time::Duration::from_secs(
            config.wallet_monitoring.dex_monitoring_interval_seconds
        );
        let mut interval_timer = tokio::time::interval(interval);

        log_info!("DEX监听循环已启动，间隔: {}秒", interval.as_secs());

        loop {
            interval_timer.tick().await;

            if let Err(e) = Self::monitor_dex_transactions(
                &config,
                &dex_transactions,
                &price_cache,
                &trade_client,
            ).await {
                log_error!("DEX监听错误: {}", e);
            }
        }
    }

    /// 监听DEX交易
    async fn monitor_dex_transactions(
        config: &AppConfig,
        dex_transactions: &Arc<RwLock<Vec<DexTransaction>>>,
        price_cache: &Arc<RwLock<PriceCache>>,
        trade_client: &Arc<SolanaTrade>,
    ) -> Result<()> {
        let monitored_dex_types = &config.wallet_monitoring.monitored_dex_types;
        
        for dex_type_str in monitored_dex_types {
            if let Ok(dex_type) = DexType::from_str(dex_type_str) {
                if let Err(e) = Self::monitor_specific_dex(
                    dex_type,
                    dex_transactions,
                    price_cache,
                    trade_client,
                ).await {
                    log_error!("监听DEX {} 失败: {}", dex_type, e);
                }
            } else {
                log_warn!("不支持的DEX类型: {}", dex_type_str);
            }
        }

        Ok(())
    }

    /// 监听特定DEX的交易
    async fn monitor_specific_dex(
        dex_type: DexType,
        dex_transactions: &Arc<RwLock<Vec<DexTransaction>>>,
        price_cache: &Arc<RwLock<PriceCache>>,
        trade_client: &Arc<SolanaTrade>,
    ) -> Result<()> {
        match dex_type {
            DexType::RaydiumCpmm => {
                Self::monitor_raydium_cpmm(dex_transactions, price_cache, trade_client).await
            }
            DexType::PumpFun => {
                Self::monitor_pumpfun(dex_transactions, price_cache, trade_client).await
            }
            DexType::PumpSwap => {
                Self::monitor_pumpswap(dex_transactions, price_cache, trade_client).await
            }
            DexType::Bonk => {
                Self::monitor_bonk(dex_transactions, price_cache, trade_client).await
            }
        }
    }

    /// 监听Raydium CPMM交易
    async fn monitor_raydium_cpmm(
        dex_transactions: &Arc<RwLock<Vec<DexTransaction>>>,
        price_cache: &Arc<RwLock<PriceCache>>,
        trade_client: &Arc<SolanaTrade>,
    ) -> Result<()> {
        // 这里应该实现具体的Raydium CPMM交易监听逻辑
        // 可以通过RPC查询最近的交易，或者使用WebSocket连接
        // 简化实现：模拟交易数据
        
        let mock_transaction = DexTransaction {
            dex_type: DexType::RaydiumCpmm,
            token_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(), // USDC
            transaction_type: TransactionType::Buy,
            amount_sol: 100_000_000, // 0.1 SOL
            amount_token: 1_000_000, // 1 USDC
            timestamp: Utc::now(),
            signature: "mock_signature_raydium".to_string(),
            pool_address: Some("mock_pool_address".to_string()),
            price_sol_per_token: 0.0001, // 0.0001 SOL per USDC
        };

        // 添加到DEX交易记录
        {
            let mut transactions = dex_transactions.write().await;
            transactions.push(mock_transaction.clone());
            
            // 限制记录数量，避免内存泄漏
            if transactions.len() > 1000 {
                transactions.drain(0..100);
            }
        }

        // 更新价格缓存
        Self::update_price_cache(
            price_cache,
            &mock_transaction.token_mint,
            mock_transaction.price_sol_per_token,
            &dex_type.to_string(),
        ).await;

        log_debug!("Raydium CPMM: 监控到交易，Token: {}, 价格: {} SOL", 
            mock_transaction.token_mint, mock_transaction.price_sol_per_token);

        Ok(())
    }

    /// 监听PumpFun交易
    async fn monitor_pumpfun(
        dex_transactions: &Arc<RwLock<Vec<DexTransaction>>>,
        price_cache: &Arc<RwLock<PriceCache>>,
        trade_client: &Arc<SolanaTrade>,
    ) -> Result<()> {
        // 实现PumpFun交易监听逻辑
        // 简化实现：模拟交易数据
        let mock_transaction = DexTransaction {
            dex_type: DexType::PumpFun,
            token_mint: "mock_token_pumpfun".to_string(),
            transaction_type: TransactionType::Buy,
            amount_sol: 50_000_000, // 0.05 SOL
            amount_token: 1000,
            timestamp: Utc::now(),
            signature: "mock_signature_pumpfun".to_string(),
            pool_address: None,
            price_sol_per_token: 0.00005, // 0.00005 SOL per token
        };

        // 添加到DEX交易记录
        {
            let mut transactions = dex_transactions.write().await;
            transactions.push(mock_transaction.clone());
            
            if transactions.len() > 1000 {
                transactions.drain(0..100);
            }
        }

        // 更新价格缓存
        Self::update_price_cache(
            price_cache,
            &mock_transaction.token_mint,
            mock_transaction.price_sol_per_token,
            &dex_type.to_string(),
        ).await;

        log_debug!("PumpFun: 监控到交易，Token: {}, 价格: {} SOL", 
            mock_transaction.token_mint, mock_transaction.price_sol_per_token);

        Ok(())
    }

    /// 监听PumpSwap交易
    async fn monitor_pumpswap(
        dex_transactions: &Arc<RwLock<Vec<DexTransaction>>>,
        price_cache: &Arc<RwLock<PriceCache>>,
        trade_client: &Arc<SolanaTrade>,
    ) -> Result<()> {
        // 实现PumpSwap交易监听逻辑
        // 简化实现：模拟交易数据
        let mock_transaction = DexTransaction {
            dex_type: DexType::PumpSwap,
            token_mint: "mock_token_pumpswap".to_string(),
            transaction_type: TransactionType::Buy,
            amount_sol: 75_000_000, // 0.075 SOL
            amount_token: 500,
            timestamp: Utc::now(),
            signature: "mock_signature_pumpswap".to_string(),
            pool_address: None,
            price_sol_per_token: 0.00015, // 0.00015 SOL per token
        };

        // 添加到DEX交易记录
        {
            let mut transactions = dex_transactions.write().await;
            transactions.push(mock_transaction.clone());
            
            if transactions.len() > 1000 {
                transactions.drain(0..100);
            }
        }

        // 更新价格缓存
        Self::update_price_cache(
            price_cache,
            &mock_transaction.token_mint,
            mock_transaction.price_sol_per_token,
            &dex_type.to_string(),
        ).await;

        log_debug!("PumpSwap: 监控到交易，Token: {}, 价格: {} SOL", 
            mock_transaction.token_mint, mock_transaction.price_sol_per_token);

        Ok(())
    }

    /// 监听Bonk交易
    async fn monitor_bonk(
        dex_transactions: &Arc<RwLock<Vec<DexTransaction>>>,
        price_cache: &Arc<RwLock<PriceCache>>,
        trade_client: &Arc<SolanaTrade>,
    ) -> Result<()> {
        // 实现Bonk交易监听逻辑
        // 简化实现：模拟交易数据
        let mock_transaction = DexTransaction {
            dex_type: DexType::Bonk,
            token_mint: "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string(), // BONK
            transaction_type: TransactionType::Buy,
            amount_sol: 25_000_000, // 0.025 SOL
            amount_token: 10000,
            timestamp: Utc::now(),
            signature: "mock_signature_bonk".to_string(),
            pool_address: None,
            price_sol_per_token: 0.0000025, // 0.0000025 SOL per BONK
        };

        // 添加到DEX交易记录
        {
            let mut transactions = dex_transactions.write().await;
            transactions.push(mock_transaction.clone());
            
            if transactions.len() > 1000 {
                transactions.drain(0..100);
            }
        }

        // 更新价格缓存
        Self::update_price_cache(
            price_cache,
            &mock_transaction.token_mint,
            mock_transaction.price_sol_per_token,
            &dex_type.to_string(),
        ).await;

        log_debug!("Bonk: 监控到交易，Token: {}, 价格: {} SOL", 
            mock_transaction.token_mint, mock_transaction.price_sol_per_token);

        Ok(())
    }

    /// 更新价格缓存
    async fn update_price_cache(
        price_cache: &Arc<RwLock<PriceCache>>,
        token_mint: &str,
        price_sol: f64,
        source: &str,
    ) {
        let mut prices = price_cache.write().await;
        let now = Utc::now();
        prices.last_update = now;

        if let Some(token_price) = prices.prices.get_mut(token_mint) {
            token_price.price_sol = price_sol;
            token_price.source = source.to_string();
            token_price.timestamp = now;
            log_debug!("更新价格缓存: Token={}, 价格={} SOL, 来源={}", token_mint, price_sol, source);
        } else {
            let new_price = TokenPrice {
                token_mint: token_mint.to_string(),
                price_sol,
                price_usd: None,
                volume_24h: None,
                timestamp: now,
                source: source.to_string(),
            };
            prices.prices.insert(token_mint.to_string(), new_price);
            log_debug!("添加价格缓存: Token={}, 价格={} SOL, 来源={}", token_mint, price_sol, source);
        }
    }
}
