use crate::{
    common::{AppConfig, log_info, log_warn, log_error, log_debug},
    trading::raydium_cpmm::common::calculate_price,
    SolanaTrade,
};
use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use std::str::FromStr;

/// 价格记录
#[derive(Debug, Clone)]
pub struct PriceRecord {
    pub token_mint: String,
    pub price: f64,
    pub timestamp: DateTime<Utc>,
    pub pool_state: String,
}

/// 持仓记录
#[derive(Debug, Clone)]
pub struct Position {
    pub token_mint: String,
    pub amount_token: u64,
    pub buy_price: f64,
    pub buy_timestamp: DateTime<Utc>,
    pub total_cost_sol: u64,
}

/// 价格监控器
pub struct PriceMonitor {
    config: Arc<AppConfig>,
    trade_client: Arc<SolanaTrade>,
    price_cache: Arc<RwLock<HashMap<String, PriceRecord>>>,
    positions: Arc<RwLock<HashMap<String, Position>>>,
}

impl PriceMonitor {
    /// 创建新的价格监控器
    pub fn new(config: Arc<AppConfig>, trade_client: Arc<SolanaTrade>) -> Self {
        Self {
            config,
            trade_client,
            price_cache: Arc::new(RwLock::new(HashMap::new())),
            positions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 更新Token价格
    pub async fn update_price(
        &self,
        token_mint: &str,
        price: f64,
        pool_state: &str,
    ) -> Result<()> {
        let price_record = PriceRecord {
            token_mint: token_mint.to_string(),
            price,
            timestamp: Utc::now(),
            pool_state: pool_state.to_string(),
        };

        let mut cache = self.price_cache.write().await;
        cache.insert(token_mint.to_string(), price_record.clone());

        log_debug!(
            "更新价格: Token={}, 价格={}, 时间={}",
            token_mint,
            price,
            price_record.timestamp.format("%H:%M:%S")
        );

        // 检查是否需要触发止盈止损
        self.check_take_profit_stop_loss(token_mint, price).await?;

        Ok(())
    }

    /// 添加持仓
    pub async fn add_position(
        &self,
        token_mint: &str,
        amount_token: u64,
        buy_price: f64,
        total_cost_sol: u64,
    ) -> Result<()> {
        let position = Position {
            token_mint: token_mint.to_string(),
            amount_token,
            buy_price,
            buy_timestamp: Utc::now(),
            total_cost_sol,
        };

        let mut positions = self.positions.write().await;
        positions.insert(token_mint.to_string(), position.clone());

        log_info!(
            "添加持仓: Token={}, 数量={}, 买入价格={}, 成本={} SOL",
            token_mint,
            amount_token,
            buy_price,
            total_cost_sol as f64 / 1_000_000_000.0
        );

        Ok(())
    }

    /// 更新持仓
    pub async fn update_position(
        &self,
        token_mint: &str,
        amount_token: u64,
        buy_price: f64,
        total_cost_sol: u64,
    ) -> Result<()> {
        let mut positions = self.positions.write().await;
        if let Some(position) = positions.get_mut(token_mint) {
            position.amount_token = amount_token;
            position.buy_price = buy_price;
            position.total_cost_sol = total_cost_sol;
            position.buy_timestamp = Utc::now();

            log_info!(
                "更新持仓: Token={}, 数量={}, 买入价格={}, 成本={} SOL",
                token_mint,
                amount_token,
                buy_price,
                total_cost_sol as f64 / 1_000_000_000.0
            );
        } else {
            // 如果持仓不存在，则添加新持仓
            self.add_position(token_mint, amount_token, buy_price, total_cost_sol).await?;
        }

        Ok(())
    }

    /// 移除持仓
    pub async fn remove_position(&self, token_mint: &str) -> Result<()> {
        let mut positions = self.positions.write().await;
        if positions.remove(token_mint).is_some() {
            log_info!("移除持仓: Token={}", token_mint);
        }

        Ok(())
    }

    /// 检查止盈止损
    pub async fn check_take_profit_stop_loss(&self, token_mint: &str, current_price: f64) -> Result<()> {
        if !self.config.take_profit_stop_loss.enabled {
            return Ok(());
        }

        let positions = self.positions.read().await;
        let position = match positions.get(token_mint) {
            Some(pos) => pos,
            None => return Ok(()),
        };

        let buy_price = position.buy_price;
        let price_change_percentage = ((current_price - buy_price) / buy_price) * 100.0;

        log_debug!(
            "检查止盈止损: Token={}, 买入价格={}, 当前价格={}, 变化={:.2}%",
            token_mint,
            buy_price,
            current_price,
            price_change_percentage
        );

        // 检查止盈
        if price_change_percentage >= self.config.take_profit_stop_loss.take_profit_percentage {
            log_info!(
                "触发止盈: Token={}, 价格变化={:.2}% >= {}%",
                token_mint,
                price_change_percentage,
                self.config.take_profit_stop_loss.take_profit_percentage
            );
            
            self.trigger_sell(token_mint, "止盈").await?;
        }

        // 检查止损
        if price_change_percentage <= -self.config.take_profit_stop_loss.stop_loss_percentage {
            log_warn!(
                "触发止损: Token={}, 价格变化={:.2}% <= -{}%",
                token_mint,
                price_change_percentage,
                self.config.take_profit_stop_loss.stop_loss_percentage
            );
            
            self.trigger_sell(token_mint, "止损").await?;
        }

        Ok(())
    }

    /// 触发卖出操作
    async fn trigger_sell(&self, token_mint: &str, reason: &str) -> Result<()> {
        let positions = self.positions.read().await;
        let position = match positions.get(token_mint) {
            Some(pos) => pos,
            None => return Ok(()),
        };

        log_info!(
            "触发卖出: Token={}, 原因={}, 数量={}, 买入价格={}",
            token_mint,
            reason,
            position.amount_token,
            position.buy_price
        );

        // 执行卖出交易
        match self.execute_sell(token_mint, position.amount_token, reason).await {
            Ok(_) => {
                log_info!("卖出交易执行成功: Token={}, 原因={}", token_mint, reason);
                // 移除持仓
                self.remove_position(token_mint).await?;
            }
            Err(e) => {
                log_error!("卖出交易执行失败: Token={}, 原因={}, 错误: {}", token_mint, reason, e);
            }
        }

        Ok(())
    }

    /// 执行卖出交易
    async fn execute_sell(&self, token_mint: &str, amount_token: u64, reason: &str) -> Result<()> {
        let mint_pubkey = match solana_sdk::pubkey::Pubkey::from_str(token_mint) {
            Ok(pubkey) => pubkey,
            Err(e) => return Err(anyhow::anyhow!("无效的Token地址: {}, 错误: {}", token_mint, e)),
        };

        let recent_blockhash = self.trade_client.rpc.get_latest_blockhash().await?;
        let slippage_basis_points = Some(self.config.trading.default_slippage_basis_points);

        // 使用Raydium CPMM执行卖出
        let raydium_params = crate::trading::core::params::RaydiumCpmmParams {
            pool_state: None, // 自动计算
            mint_token_program: Some(spl_token::ID),
            mint_token_in_pool_state_index: Some(1),
            minimum_amount_out: None,
            auto_handle_wsol: self.config.trading.auto_handle_wsol,
        };

        self.trade_client.sell(
            crate::trading::factory::DexType::RaydiumCpmm,
            mint_pubkey,
            None,
            amount_token,
            slippage_basis_points,
            recent_blockhash,
            None,
            false,
            Some(Box::new(raydium_params)),
        ).await?;

        Ok(())
    }

    /// 获取当前价格
    pub async fn get_current_price(&self, token_mint: &str) -> Option<f64> {
        let cache = self.price_cache.read().await;
        cache.get(token_mint).map(|record| record.price)
    }

    /// 获取持仓信息
    pub async fn get_position(&self, token_mint: &str) -> Option<Position> {
        let positions = self.positions.read().await;
        positions.get(token_mint).cloned()
    }

    /// 获取所有持仓
    pub async fn get_all_positions(&self) -> Vec<Position> {
        let positions = self.positions.read().await;
        positions.values().cloned().collect()
    }

    /// 清理过期的价格缓存
    pub async fn cleanup_expired_cache(&self) -> Result<()> {
        let now = Utc::now();
        let mut cache = self.price_cache.write().await;
        
        let expired_keys: Vec<String> = cache
            .iter()
            .filter(|(_, record)| {
                now.signed_duration_since(record.timestamp).num_seconds() > self.config.monitoring.price_cache_ttl_seconds as i64
            })
            .map(|(key, _)| key.clone())
            .collect();

        for key in expired_keys {
            cache.remove(&key);
        }

        if !expired_keys.is_empty() {
            log_debug!("清理了 {} 个过期价格缓存", expired_keys.len());
        }

        Ok(())
    }

    /// 计算持仓的当前价值
    pub async fn calculate_position_value(&self, token_mint: &str) -> Option<f64> {
        let position = self.get_position(token_mint).await?;
        let current_price = self.get_current_price(token_mint).await?;
        
        Some(current_price * position.amount_token as f64)
    }

    /// 计算持仓的盈亏百分比
    pub async fn calculate_position_pnl_percentage(&self, token_mint: &str) -> Option<f64> {
        let position = self.get_position(token_mint).await?;
        let current_price = self.get_current_price(token_mint).await?;
        
        let pnl_percentage = ((current_price - position.buy_price) / position.buy_price) * 100.0;
        Some(pnl_percentage)
    }

    /// 获取所有持仓的汇总信息
    pub async fn get_positions_summary(&self) -> PositionsSummary {
        let positions = self.get_all_positions().await;
        let mut total_cost_sol = 0u64;
        let mut total_current_value = 0.0;
        let mut total_pnl = 0.0;

        for position in &positions {
            total_cost_sol += position.total_cost_sol;
            
            if let Some(current_price) = self.get_current_price(&position.token_mint).await {
                let current_value = current_price * position.amount_token as f64;
                total_current_value += current_value;
                
                let position_pnl = (current_price - position.buy_price) * position.amount_token as f64;
                total_pnl += position_pnl;
            }
        }

        let total_cost_sol_f64 = total_cost_sol as f64 / 1_000_000_000.0;
        let total_pnl_percentage = if total_cost_sol_f64 > 0.0 {
            (total_pnl / total_cost_sol_f64) * 100.0
        } else {
            0.0
        };

        PositionsSummary {
            total_positions: positions.len(),
            total_cost_sol: total_cost_sol_f64,
            total_current_value,
            total_pnl,
            total_pnl_percentage,
        }
    }
}

/// 持仓汇总信息
#[derive(Debug, Clone)]
pub struct PositionsSummary {
    pub total_positions: usize,
    pub total_cost_sol: f64,
    pub total_current_value: f64,
    pub total_pnl: f64,
    pub total_pnl_percentage: f64,
}
