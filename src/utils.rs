use crate::solana_streamer_sdk::streaming::event_parser::protocols::pumpfun::PumpFunTradeEvent;
use crate::trading;
use crate::SolanaTrade;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use std::fs::OpenOptions;
use std::io::Write;
use chrono::{DateTime, Utc};
use std::sync::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    static ref LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
}

/// 初始化日志文件
pub fn init_log_file(filename: &str) -> Result<(), std::io::Error> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(filename)?;
    
    let mut log_file = LOG_FILE.lock().unwrap();
    *log_file = Some(file);
    Ok(())
}

/// 写入日志到文件
pub fn write_log(message: &str) {
    let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let log_message = format!("[{}] {}\n", timestamp, message);
    
    // 同时输出到控制台和文件
    println!("{}", message);
    
    if let Ok(mut log_file) = LOG_FILE.lock() {
        if let Some(ref mut file) = *log_file {
            let _ = file.write_all(log_message.as_bytes());
            let _ = file.flush();
        }
    }
}

/// 写入日志到文件（不输出到控制台）
pub fn write_log_only(message: &str) {
    let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let log_message = format!("[{}] {}\n", timestamp, message);
    
    if let Ok(mut log_file) = LOG_FILE.lock() {
        if let Some(ref mut file) = *log_file {
            let _ = file.write_all(log_message.as_bytes());
            let _ = file.flush();
        }
    }
}

/// 获取当前时间戳字符串
pub fn get_current_timestamp() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

impl SolanaTrade {
    #[inline]
    pub async fn get_sol_balance(&self, payer: &Pubkey) -> Result<u64, anyhow::Error> {
        trading::common::utils::get_sol_balance(&self.rpc, payer).await
    }

    #[inline]
    pub async fn get_payer_sol_balance(&self) -> Result<u64, anyhow::Error> {
        trading::common::utils::get_sol_balance(&self.rpc, &self.payer.pubkey()).await
    }

    #[inline]
    pub async fn get_token_balance(
        &self,
        payer: &Pubkey,
        mint: &Pubkey,
    ) -> Result<u64, anyhow::Error> {
        trading::common::utils::get_token_balance(&self.rpc, payer, mint).await
    }

    #[inline]
    pub async fn get_payer_token_balance(&self, mint: &Pubkey) -> Result<u64, anyhow::Error> {
        trading::common::utils::get_token_balance(&self.rpc, &self.payer.pubkey(), mint).await
    }

    #[inline]
    pub fn get_payer_pubkey(&self) -> Pubkey {
        self.payer.pubkey()
    }

    #[inline]
    pub fn get_payer(&self) -> &Keypair {
        self.payer.as_ref()
    }

    #[inline]
    pub async fn transfer_sol(
        &self,
        payer: &Keypair,
        receive_wallet: &Pubkey,
        amount: u64,
    ) -> Result<(), anyhow::Error> {
        trading::common::utils::transfer_sol(&self.rpc, payer, receive_wallet, amount).await
    }

    #[inline]
    pub async fn close_token_account(&self, mint: &Pubkey) -> Result<(), anyhow::Error> {
        trading::common::utils::close_token_account(&self.rpc, self.payer.as_ref(), mint).await
    }

    // -------------------------------- PumpFun --------------------------------

    #[inline]
    pub fn get_pumpfun_token_price(
        &self,
        virtual_sol_reserves: u64,
        virtual_token_reserves: u64,
    ) -> f64 {
        trading::pumpfun::common::get_token_price(virtual_sol_reserves, virtual_token_reserves)
    }

    #[inline]
    pub fn get_pumpfun_token_buy_price(&self, amount: u64, trade_info: &PumpFunTradeEvent) -> u64 {
        trading::pumpfun::common::get_buy_price(amount, trade_info)
    }

    #[inline]
    pub async fn get_pumpfun_token_current_price(
        &self,
        mint: &Pubkey,
    ) -> Result<f64, anyhow::Error> {
        let (bonding_curve, _) =
            trading::pumpfun::common::get_bonding_curve_account_v2(&self.rpc, mint).await?;

        let virtual_sol_reserves = bonding_curve.virtual_sol_reserves;
        let virtual_token_reserves = bonding_curve.virtual_token_reserves;

        Ok(trading::pumpfun::common::get_token_price(
            virtual_sol_reserves,
            virtual_token_reserves,
        ))
    }

    #[inline]
    pub async fn get_pumpfun_token_real_sol_reserves(
        &self,
        mint: &Pubkey,
    ) -> Result<u64, anyhow::Error> {
        let (bonding_curve, _) =
            trading::pumpfun::common::get_bonding_curve_account_v2(&self.rpc, mint).await?;

        let actual_sol_reserves = bonding_curve.real_sol_reserves;

        Ok(actual_sol_reserves)
    }

    #[inline]
    pub async fn get_pumpfun_token_creator(&self, mint: &Pubkey) -> Result<Pubkey, anyhow::Error> {
        let (bonding_curve, _) =
            trading::pumpfun::common::get_bonding_curve_account_v2(&self.rpc, mint).await?;

        let creator = bonding_curve.creator;

        Ok(creator)
    }

    // -------------------------------- PumpSwap --------------------------------

    #[inline]
    pub async fn get_pumpswap_token_current_price(
        &self,
        pool_address: &Pubkey,
    ) -> Result<f64, anyhow::Error> {
        let pool = trading::pumpswap::pool::Pool::fetch(&self.rpc, pool_address).await?;

        let (base_amount, quote_amount) = pool.get_token_balances(&self.rpc).await?;

        // Calculate price using constant product formula (x * y = k)
        // Price = quote_amount / base_amount
        if base_amount == 0 {
            return Err(anyhow::anyhow!(
                "Base amount is zero, cannot calculate price"
            ));
        }

        let price = quote_amount as f64 / base_amount as f64;

        Ok(price)
    }

    #[inline]
    pub async fn get_pumpswap_token_real_sol_reserves(
        &self,
        pool_address: &Pubkey,
    ) -> Result<u64, anyhow::Error> {
        let pool = trading::pumpswap::pool::Pool::fetch(&self.rpc, pool_address).await?;

        let (_, quote_amount) = pool.get_token_balances(&self.rpc).await?;

        Ok(quote_amount)
    }

    #[inline]
    pub async fn get_pumpswap_payer_token_balance(
        &self,
        pool_address: &Pubkey,
    ) -> Result<u64, anyhow::Error> {
        let pool = trading::pumpswap::pool::Pool::fetch(&self.rpc, pool_address).await?;

        let (base_amount, _) = pool.get_token_balances(&self.rpc).await?;

        Ok(base_amount)
    }

    // -------------------------------- Bonk --------------------------------

    #[inline]
    pub fn get_bonk_token_price(
        &self,
        virtual_base: u128,
        virtual_quote: u128,
        real_base: u128,
        real_quote: u128,
        decimal_base: u64,
        decimal_quote: u64,
    ) -> f64 {
        trading::bonk::common::get_token_price(
            virtual_base,
            virtual_quote,
            real_base,
            real_quote,
            decimal_base,
            decimal_quote,
        )
    }
}
