use crate::{
    common::{AppConfig, log_info, log_warn, log_error, log_debug},
    trading::{factory::DexType, core::params::RaydiumCpmmParams},
    SolanaTrade,
};
use anyhow::Result;
use solana_sdk::{pubkey::Pubkey, hash::Hash};
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use anyhow;
use std::str::FromStr;

/// 交易信号
#[derive(Debug, Clone)]
pub enum TradeSignal {
    Buy {
        token_mint: String,
        amount_sol: u64,
        pool_state: Option<String>,
    },
    Sell {
        token_mint: String,
        amount_token: u64,
        reason: String,
        pool_state: Option<String>,
    },
}

/// 策略执行状态
#[derive(Debug, Clone)]
pub enum StrategyStatus {
    Idle,
    Executing,
    Completed,
    Failed(String),
}

/// 策略执行记录
#[derive(Debug, Clone)]
pub struct StrategyExecution {
    pub id: String,
    pub token_mint: String,
    pub signal: TradeSignal,
    pub status: StrategyStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub error_message: Option<String>,
}

/// 止盈止损策略执行器
pub struct TakeProfitStopLossStrategy {
    config: Arc<AppConfig>,
    trade_client: Arc<SolanaTrade>,
    executions: Arc<RwLock<Vec<StrategyExecution>>>,
    is_running: Arc<RwLock<bool>>,
}

impl TakeProfitStopLossStrategy {
    /// 创建新的策略执行器
    pub fn new(config: Arc<AppConfig>, trade_client: Arc<SolanaTrade>) -> Self {
        Self {
            config,
            trade_client,
            executions: Arc::new(RwLock::new(Vec::new())),
            is_running: Arc::new(RwLock::new(false)),
        }
    }

    /// 启动策略
    pub async fn start(&self) -> Result<()> {
        let mut running = self.is_running.write().await;
        if *running {
            log_warn!("策略已经在运行中");
            return Ok(());
        }

        *running = true;
        log_info!("启动止盈止损策略");

        Ok(())
    }

    /// 停止策略
    pub async fn stop(&self) -> Result<()> {
        let mut running = self.is_running.write().await;
        if !*running {
            log_warn!("策略已经停止");
            return Ok(());
        }

        *running = false;
        log_info!("停止止盈止损策略");

        Ok(())
    }

    /// 检查策略是否运行中
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    /// 执行买入信号
    pub async fn execute_buy_signal(&self, signal: TradeSignal) -> Result<()> {
        if !self.is_running().await {
            log_warn!("策略未运行，忽略买入信号");
            return Ok(());
        }

        let (token_mint, amount_sol, pool_state) = match &signal {
            TradeSignal::Buy { token_mint, amount_sol, pool_state } => {
                (token_mint.clone(), *amount_sol, pool_state.clone())
            }
            _ => return Err(anyhow::anyhow!("无效的买入信号")),
        };

        log_info!(
            "执行买入信号: Token={}, 金额={} SOL",
            token_mint,
            amount_sol as f64 / 1_000_000_000.0
        );

        // 执行买入交易
        match self.execute_buy(&token_mint, amount_sol, pool_state).await {
            Ok(_) => {
                log_info!("买入执行成功: Token={}", token_mint);
            }
            Err(e) => {
                log_error!("买入执行失败: {}", e);
            }
        }

        Ok(())
    }

    /// 执行卖出信号
    pub async fn execute_sell_signal(&self, signal: TradeSignal) -> Result<()> {
        if !self.is_running().await {
            log_warn!("策略未运行，忽略卖出信号");
            return Ok(());
        }

        let (token_mint, amount_token, reason, pool_state) = match &signal {
            TradeSignal::Sell { token_mint, amount_token, reason, pool_state } => {
                (token_mint.clone(), *amount_token, reason.clone(), pool_state.clone())
            }
            _ => return Err(anyhow::anyhow!("无效的卖出信号")),
        };

        // 创建执行记录
        let execution = StrategyExecution {
            id: format!("sell_{}_{}", token_mint, Utc::now().timestamp_millis()),
            token_mint: token_mint.clone(),
            signal: signal.clone(),
            status: StrategyStatus::Executing,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            error_message: None,
        };

        // 添加到执行记录
        {
            let mut executions = self.executions.write().await;
            executions.push(execution.clone());
        }

        log_info!(
            "执行卖出信号: Token={}, 数量={}, 原因={}",
            token_mint,
            amount_token,
            reason
        );

        // 执行卖出交易
        match self.execute_sell(&token_mint, amount_token, pool_state).await {
            Ok(_) => {
                self.update_execution_status(&execution.id, StrategyStatus::Completed).await?;
                log_info!("卖出执行成功: Token={}", token_mint);
            }
            Err(e) => {
                let error_msg = format!("卖出执行失败: {}", e);
                self.update_execution_status(&execution.id, StrategyStatus::Failed(error_msg.clone())).await?;
                log_error!("{}", error_msg);
            }
        }

        Ok(())
    }

    /// 执行买入交易
    async fn execute_buy(&self, token_mint: &str, amount_sol: u64, pool_state: Option<String>) -> Result<()> {
        let mint_pubkey = Pubkey::from_str(token_mint)?;
        let recent_blockhash = self.trade_client.rpc.get_latest_blockhash().await?;
        let slippage_basis_points = Some(self.config.trading.default_slippage_basis_points);

        // 构建Raydium CPMM参数
        let raydium_params = if let Some(pool) = pool_state {
            let pool_pubkey = Pubkey::from_str(&pool)?;
            Some(Box::new(RaydiumCpmmParams {
                pool_state: Some(pool_pubkey),
                mint_token_program: Some(spl_token::ID),
                mint_token_in_pool_state_index: Some(1),
                minimum_amount_out: None,
                auto_handle_wsol: self.config.trading.auto_handle_wsol,
            }))
        } else {
            None
        };

        log_info!(
            "执行买入交易: Token={}, 金额={} SOL, 滑点={} bps",
            token_mint,
            amount_sol as f64 / 1_000_000_000.0,
            slippage_basis_points.unwrap_or(0)
        );

        // 执行买入
        self.trade_client.buy(
            DexType::RaydiumCpmm,
            mint_pubkey,
            None,
            amount_sol,
            slippage_basis_points,
            recent_blockhash,
            None,
            raydium_params,
        ).await?;

        log_info!("买入交易执行成功: Token={}", token_mint);

        Ok(())
    }

    /// 执行卖出交易
    async fn execute_sell(&self, token_mint: &str, amount_token: u64, pool_state: Option<String>) -> Result<()> {
        let mint_pubkey = Pubkey::from_str(token_mint)?;
        let recent_blockhash = self.trade_client.rpc.get_latest_blockhash().await?;
        let slippage_basis_points = Some(self.config.trading.default_slippage_basis_points);

        // 构建Raydium CPMM参数
        let raydium_params = if let Some(pool) = pool_state {
            let pool_pubkey = Pubkey::from_str(&pool)?;
            Some(Box::new(RaydiumCpmmParams {
                pool_state: Some(pool_pubkey),
                mint_token_program: Some(pool_pubkey),
                mint_token_in_pool_state_index: Some(1),
                minimum_amount_out: None,
                auto_handle_wsol: self.config.trading.auto_handle_wsol,
            }))
        } else {
            None
        };

        log_info!(
            "执行卖出交易: Token={}, 数量={}, 滑点={} bps",
            token_mint,
            amount_token,
            slippage_basis_points.unwrap_or(0)
        );

        // 执行卖出
        self.trade_client.sell(
            DexType::RaydiumCpmm,
            mint_pubkey,
            None,
            amount_token,
            slippage_basis_points,
            recent_blockhash,
            None,
            false,
            raydium_params,
        ).await?;

        log_info!("卖出交易执行成功: Token={}", token_mint);

        Ok(())
    }

    /// 更新执行状态
    async fn update_execution_status(&self, execution_id: &str, status: StrategyStatus) -> Result<()> {
        let mut executions = self.executions.write().await;
        if let Some(execution) = executions.iter_mut().find(|e| e.id == execution_id) {
            execution.status = status;
            execution.updated_at = Utc::now();
        }

        Ok(())
    }

    /// 获取执行记录
    pub async fn get_executions(&self) -> Vec<StrategyExecution> {
        let executions = self.executions.read().await;
        executions.clone()
    }

    /// 清理完成的执行记录
    pub async fn cleanup_completed_executions(&self) -> Result<()> {
        let mut executions = self.executions.write().await;
        let before_count = executions.len();
        
        executions.retain(|e| {
            !matches!(e.status, StrategyStatus::Completed | StrategyStatus::Failed(_))
        });

        let after_count = executions.len();
        let cleaned_count = before_count - after_count;

        if cleaned_count > 0 {
            log_debug!("清理了 {} 条已完成的执行记录", cleaned_count);
        }

        Ok(())
    }

    /// 获取策略统计信息
    pub async fn get_strategy_stats(&self) -> StrategyStats {
        let executions = self.executions.read().await;
        
        let mut total_executions = 0;
        let mut completed_executions = 0;
        let mut failed_executions = 0;
        let mut pending_executions = 0;

        for execution in executions.iter() {
            total_executions += 1;
            match execution.status {
                StrategyStatus::Completed => completed_executions += 1,
                StrategyStatus::Failed(_) => failed_executions += 1,
                StrategyStatus::Executing => pending_executions += 1,
                StrategyStatus::Idle => {}
            }
        }

        let success_rate = if total_executions > 0 {
            (completed_executions as f64 / total_executions as f64) * 100.0
        } else {
            0.0
        };

        StrategyStats {
            total_executions,
            completed_executions,
            failed_executions,
            pending_executions,
            success_rate,
        }
    }

    /// 重置策略状态
    pub async fn reset_strategy(&self) -> Result<()> {
        let mut executions = self.executions.write().await;
        executions.clear();
        
        let mut running = self.is_running.write().await;
        *running = false;

        log_info!("策略状态已重置");
        Ok(())
    }
}

/// 策略统计信息
#[derive(Debug, Clone)]
pub struct StrategyStats {
    pub total_executions: usize,
    pub completed_executions: usize,
    pub failed_executions: usize,
    pub pending_executions: usize,
    pub success_rate: f64,
}
