use std::{str::FromStr, sync::Arc};

// 导入交易策略模块
mod strategy;
use strategy::take_profit_stop_loss::{test_take_profit_stop_loss_strategy, test_basic_functionality, test_price_calculation};

use sol_trade_sdk::{
    common::{bonding_curve::BondingCurveAccount, AnyResult, PriorityFee, TradeConfig, config::Config},
    swqos::{SwqosConfig, SwqosRegion},
    trading::{core::params::{BonkParams, PumpFunParams, RaydiumCpmmParams}, factory::DexType, raydium_cpmm::{common::{get_buy_token_amount, get_sell_sol_amount}}},
    SolanaTrade, utils,
};
use sol_trade_sdk::solana_streamer_sdk::{
    match_event,
    streaming::{
        event_parser::{
            protocols::{
                bonk::{BonkPoolCreateEvent, BonkTradeEvent},
                pumpfun::{PumpFunCreateTokenEvent, PumpFunTradeEvent},
                pumpswap::{
                    PumpSwapBuyEvent, PumpSwapCreatePoolEvent, PumpSwapDepositEvent,
                    PumpSwapSellEvent, PumpSwapWithdrawEvent,
                }, raydium_cpmm::RaydiumCpmmSwapEvent,
            },
            Protocol, UnifiedEvent,
        },
        ShredStreamGrpc, YellowstoneGrpc,
    },
};
use solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Keypair};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志文件
    let log_filename = format!("trading_log_{}.txt", chrono::Utc::now().format("%Y%m%d_%H%M%S"));
    utils::init_log_file(&log_filename)?;
    utils::write_log(&format!("🚀 交易系统启动，日志文件: {}", log_filename));
    
    // 测试止盈止损策略
    utils::write_log("开始测试止盈止损策略...");
    utils::write_log("正在调用调试友好的测试函数...");
    
    // 使用调试友好的测试函数，避免LLDB问题
    if let Err(e) = test_take_profit_stop_loss_strategy().await {
        utils::write_log(&format!("❌ 止盈止损策略测试失败: {}", e));
        utils::write_log(&format!("错误详情: {:?}", e));
        return Err(e);
    }
    
    utils::write_log("✅ 止盈止损策略测试完成");
    
    // 测试基本功能
    utils::write_log("开始测试基本功能...");
    if let Err(e) = test_basic_functionality().await {
        utils::write_log(&format!("❌ 基本功能测试失败: {}", e));
        return Err(e);
    }
    utils::write_log("✅ 基本功能测试完成");
    
    Ok(())
}

/// 创建 SolanaTrade 客户端
/// 
/// 这个函数创建了一个Solana交易客户端，用于执行各种DEX交易
/// 
/// # 配置说明
/// - 从配置文件加载钱包私钥和RPC配置
/// - 支持多种MEV服务配置
/// - 自动配置SWQoS优化服务
/// 
/// # 返回值
/// - `AnyResult<SolanaTrade>`: 成功时返回交易客户端，失败时返回错误
async fn test_create_solana_trade_client() -> AnyResult<SolanaTrade> {
    println!("正在创建 SolanaTrade 客户端...");
    
    // 从配置文件加载配置
    let config = Config::load()?;
    println!("✅ 配置文件加载成功");
    
    // 从配置文件读取私钥
    let private_key = config.get_private_key();
    let payer = Keypair::from_base58_string(private_key);
    println!("✅ 钱包私钥加载成功");
    
    // 从配置文件读取RPC配置
    let rpc_url = config.get_main_rpc_url().to_string();
    println!("✅ RPC配置加载成功: {}", rpc_url);
    
    // 从配置文件读取MEV服务配置
    let enabled_mev_services = config.get_enabled_mev_services();
    let swqos_configs = create_swqos_configs_from_config(&config, enabled_mev_services);
    
    let trade_config = create_trade_config_from_config(&config, swqos_configs);

    let solana_trade_client = SolanaTrade::new(Arc::new(payer), trade_config).await;
    println!("✅ SolanaTrade 客户端创建成功!");

    Ok(solana_trade_client)
}

/// 从配置文件创建SWQoS配置
fn create_swqos_configs_from_config(config: &Config, enabled_services: Vec<(&str, &str, Vec<&str>)>) -> Vec<SwqosConfig> {
    let mut swqos_configs = Vec::new();
    
    // 添加默认RPC配置
    swqos_configs.push(SwqosConfig::Default(config.get_main_rpc_url().to_string()));
    
    // 根据配置文件添加启用的MEV服务
    for (service_name, api_token, regions) in enabled_services {
        match service_name {
            "jito" => {
                for region in regions {
                    if let Ok(swqos_region) = region.parse::<SwqosRegion>() {
                        swqos_configs.push(SwqosConfig::Jito(swqos_region));
                    }
                }
            }
            "nextblock" => {
                for region in regions {
                    if let Ok(swqos_region) = region.parse::<SwqosRegion>() {
                        swqos_configs.push(SwqosConfig::NextBlock(api_token.to_string(), swqos_region));
                    }
                }
            }
            "bloxroute" => {
                for region in regions {
                    if let Ok(swqos_region) = region.parse::<SwqosRegion>() {
                        swqos_configs.push(SwqosConfig::Bloxroute(api_token.to_string(), swqos_region));
                    }
                }
            }
            "zeroslot" => {
                for region in regions {
                    if let Ok(swqos_region) = region.parse::<SwqosRegion>() {
                        swqos_configs.push(SwqosConfig::ZeroSlot(api_token.to_string(), swqos_region));
                    }
                }
            }
            "temporal" => {
                for region in regions {
                    if let Ok(swqos_region) = region.parse::<SwqosRegion>() {
                        swqos_configs.push(SwqosConfig::Temporal(api_token.to_string(), swqos_region));
                    }
                }
            }
            _ => {}
        }
    }
    
    swqos_configs
}

/// 从配置文件创建交易配置
fn create_trade_config_from_config(config: &Config, swqos_configs: Vec<SwqosConfig>) -> TradeConfig {
    TradeConfig::new(
        config.get_main_rpc_url().to_string(),
        swqos_configs,
        PriorityFee {
            unit_limit: config.get_priority_fees().compute_unit_limit,
            unit_price: config.get_priority_fees().compute_unit_price,
            rpc_unit_limit: config.get_priority_fees().rpc_compute_unit_limit,
            rpc_unit_price: config.get_priority_fees().rpc_compute_unit_price,
            buy_tip_fee: config.get_priority_fees().buy_tip_fee,
            buy_tip_fees: vec![],
            smart_buy_tip_fee: 0.0,
            sell_tip_fee: config.get_priority_fees().sell_tip_fee,
        },
        CommitmentConfig::confirmed(),
        None, // lookup_table_key
    )
}

async fn test_pumpfun_copy_trade_with_grpc(trade_info: PumpFunTradeEvent) -> AnyResult<()> {
    println!("Testing PumpFun trading...");

    let client = test_create_solana_trade_client().await?;
    let creator = Pubkey::from_str("xxxxxx")?;
    let mint_pubkey = Pubkey::from_str("xxxxxx")?;
    let buy_sol_cost = 100_000;
    let slippage_basis_points = Some(100);
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;
    let bonding_curve = BondingCurveAccount::from_trade(&trade_info);

    // Buy tokens
    println!("Buying tokens from PumpFun...");
    client.buy(
        DexType::PumpFun,
        mint_pubkey,
        Some(creator),
        buy_sol_cost,
        slippage_basis_points,
        recent_blockhash,
        None,
        Some(Box::new(PumpFunParams {
            bonding_curve: Some(Arc::new(bonding_curve.clone())),
        })),
    ).await?;

    // Sell tokens  
    println!("Selling tokens from PumpFun...");
    let amount_token = 0;
    client.sell(
        DexType::PumpFun,
        mint_pubkey,
        Some(creator),
        amount_token,
        slippage_basis_points,
        recent_blockhash,
        None,
        None,
    ).await?;

    Ok(())
}

async fn test_pumpfun_sniper_trade_with_shreds(trade_info: PumpFunTradeEvent) -> AnyResult<()> {
    println!("Testing PumpFun trading...");

    if !trade_info.is_dev_create_token_trade {
        return Ok(());
    }

    let client = test_create_solana_trade_client().await?;
    let mint_pubkey = trade_info.mint;
    let creator = trade_info.creator;
    let slippage_basis_points = Some(100);
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;

    let bonding_curve = BondingCurveAccount::from_dev_trade(
        &mint_pubkey,
        trade_info.token_amount,
        trade_info.max_sol_cost,
        creator,
    );

    // Buy tokens
    println!("Buying tokens from PumpFun...");
    let buy_sol_amount = 100_000;
    client.buy(
        DexType::PumpFun,
        mint_pubkey,
        Some(creator),
        buy_sol_amount,
        slippage_basis_points,
        recent_blockhash,
        None,
        Some(Box::new(PumpFunParams {
            bonding_curve: Some(Arc::new(bonding_curve.clone())),
        })),
    ).await?;

    // Sell tokens
    println!("Selling tokens from PumpFun...");
    let amount_token = 0;
    client.sell(
        DexType::PumpFun,
        mint_pubkey,
        Some(creator),
        amount_token,
        slippage_basis_points,
        recent_blockhash,
        None,
        None,
    ).await?;

    Ok(())
}

async fn test_pumpswap() -> AnyResult<()> {
    println!("Testing PumpSwap trading...");

    let client = test_create_solana_trade_client().await?;
    let creator = Pubkey::from_str("11111111111111111111111111111111")?;
    let mint_pubkey = Pubkey::from_str("2zMMhcVQEXDtdE6vsFS7S7D5oUodfJHE8vd1gnBouauv")?;
    let buy_sol_cost = 100_000;
    let slippage_basis_points = Some(100);
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;

    // Buy tokens
    println!("Buying tokens from PumpSwap...");
    client.buy(
        DexType::PumpSwap,
        mint_pubkey,
        Some(creator),
        buy_sol_cost,
        slippage_basis_points,
        recent_blockhash,
        None,
        None,
    ).await?;

    // Sell tokens
    println!("Selling tokens from PumpSwap...");
    let amount_token = 0;
    client.sell(
        DexType::PumpSwap,
        mint_pubkey,
        Some(creator),
        amount_token,
        slippage_basis_points,
        recent_blockhash,
        None,
        None,
    ).await?;

    Ok(())
}

async fn test_bonk_copy_trade_with_grpc(trade_info: BonkTradeEvent) -> AnyResult<()> {
    println!("Testing Bonk trading...");

    let client = test_create_solana_trade_client().await?;
    let mint_pubkey = Pubkey::from_str("xxxxxxx")?;
    let buy_sol_cost = 100_000;
    let slippage_basis_points = Some(100);
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;

    // Buy tokens
    println!("Buying tokens from letsbonk.fun...");
    client.buy(
        DexType::Bonk,
        mint_pubkey,
        None,
        buy_sol_cost,
        slippage_basis_points,
        recent_blockhash,
        None,
        Some(Box::new(BonkParams::from_trade(trade_info))),
    ).await?;

    // Sell tokens
    println!("Selling tokens from letsbonk.fun...");
    let amount_token = 0;
    client.sell(
        DexType::Bonk,
        mint_pubkey,
        None,
        amount_token,
        slippage_basis_points,
        recent_blockhash,
        None,
        None,
    ).await?;

    Ok(())
}

async fn test_bonk_sniper_trade_with_shreds(trade_info: BonkTradeEvent) -> AnyResult<()> {
    println!("Testing Bonk trading...");

    if !trade_info.is_dev_create_token_trade {
        return Ok(());
    }

    let client = test_create_solana_trade_client().await?;
    let mint_pubkey = Pubkey::from_str("xxxxxxx")?;
    let buy_sol_cost = 100_000;
    let slippage_basis_points = Some(100);
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;

    // Buy tokens
    println!("Buying tokens from letsbonk.fun...");
    client.buy(
        DexType::Bonk,
        mint_pubkey,
        None,
        buy_sol_cost,
        slippage_basis_points,
        recent_blockhash,
        None,
        Some(Box::new(BonkParams::from_dev_trade(trade_info))),
    ).await?;

    // Sell tokens
    println!("Selling tokens from letsbonk.fun...");
    let amount_token = 0;
    client.sell(
        DexType::Bonk,
        mint_pubkey,
        None,
        amount_token,
        slippage_basis_points,
        recent_blockhash,
        None,
        None,
    ).await?;

    Ok(())
}


async fn test_bonk() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing Bonk trading...");

    let client = test_create_solana_trade_client().await?;
    let mint_pubkey = Pubkey::from_str("xxxxxxx")?;
    let buy_sol_cost = 100_000;
    let slippage_basis_points = Some(100);
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;

    // Buy tokens
    println!("Buying tokens from letsbonk.fun...");
    client.buy(
        DexType::Bonk,
        mint_pubkey,
        None,
        buy_sol_cost,
        slippage_basis_points,
        recent_blockhash,
        None,
        None,
    ).await?;

    // Sell tokens
    println!("Selling tokens from letsbonk.fun...");
    let amount_token = 0;
    client.sell(
        DexType::Bonk,
        mint_pubkey,
        None,
        amount_token,
        slippage_basis_points,
        recent_blockhash,
        None,
        None,
    ).await?;

    Ok(())
}


/// 测试Raydium CPMM (Constant Product Market Maker) 交易功能
/// 
/// 这个函数演示了如何在Raydium DEX上进行代币买卖交易
/// CPMM是恒定乘积做市商模型，类似于Uniswap V2
/// 
/// # 功能说明
/// - 创建Solana交易客户端
/// - 配置交易参数（代币地址、金额、滑点等）
/// - 执行买入操作
/// - 执行卖出操作
/// 
/// # 返回值
/// - `Result<(), Box<dyn std::error::Error>>`: 异步结果类型，成功返回`()`，失败返回错误
/// 
/// # 错误处理
/// - 使用`?`操作符进行错误传播
/// - 如果任何步骤失败，函数会立即返回错误
async fn test_trade_raydium_cpmm() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing Raydium Cpmm trading...");

    // 创建Solana交易客户端
    // 这个客户端包含了RPC连接、钱包配置、SWQoS设置等
    let client = test_create_solana_trade_client().await?;
    
    // 代币的Mint地址 - 这是代币在Solana上的唯一标识符
    // Pubkey::from_str() 将字符串转换为公钥对象
    // 这里使用BONK代币作为示例，您可以替换为其他代币
    let mint_pubkey = Pubkey::from_str("6p6xgHyF7AeE6TZkSmFsko444wqoP15icUSqi2jfGiPN")?; // BONK代币
    
    // 买入时使用的SOL数量（以lamports为单位）
    let buy_sol_cost = 10_000_000;  // 0.01 SOL
    
    // 滑点容差设置（以基点为单位）
    // Some(100) 表示允许1%的滑点
    let slippage_basis_points = Some(100);
    
    // 获取最新的区块哈希
    // 每个Solana交易都需要一个有效的区块哈希
    // 区块哈希用于防止重放攻击
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;
    
    // Raydium流动性池的状态地址
    // 这个地址包含了池子的配置信息，如代币余额、费用等
    // BONK/SOL池的地址（示例）
    let pool_state = Pubkey::from_str("HKuJrP5tYQLbEUdjKwjgnHs2957QKjR2iWhJKTtMa1xs")?; // BONK/SOL池
    
    // 计算买入代币的预期数量
    // 基于当前池状态和输入的SOL数量计算能获得多少代币
    // 这个值用于设置最小输出数量，防止滑点过大
    let buy_amount_out = get_buy_token_amount(&client.rpc, &pool_state, buy_sol_cost).await?;
    
    // ========== 买入操作 ==========
    println!("Buying tokens from Raydium Cpmm...");
    
    // 执行买入交易
    // client.buy() 是SDK提供的买入方法
    client.buy(
        DexType::RaydiumCpmm,  // 指定使用Raydium CPMM协议
        mint_pubkey,           // 要买入的代币Mint地址
        None,                  // 创建者地址（对于CPMM通常为None）
        buy_sol_cost,          // 用于买入的SOL数量
        slippage_basis_points, // 滑点设置
        recent_blockhash,      // 最新区块哈希
        None,                  // 自定义指令（通常为None）
        Some(Box::new(RaydiumCpmmParams {  // Raydium CPMM特定参数
            pool_state: Some(pool_state),   // 池状态地址
            mint_token_program: Some(spl_token::ID),  // 代币程序ID（SPL Token）
            mint_token_in_pool_state_index: Some(1), // 代币在池状态中的索引位置
            minimum_amount_out: Some(buy_amount_out), // 最小输出数量（防止滑点）
            auto_handle_wsol: true,         // 自动处理wSOL转换
        })),
    ).await?;  // 等待交易完成，如果失败则返回错误

    println!("买入交易完成！");
    
    // 注释掉卖出操作，避免重复交易
    // 如果需要测试卖出，请取消注释下面的代码
    
    // ========== 卖出操作 ==========
    println!("Selling tokens from Raydium Cpmm...");
    
    // 要卖出的代币数量（以最小单位计算）
    let amount_token = 0;  // 0 表示卖出所有持有的代币
    
    // 计算卖出代币能获得的SOL数量
    let sell_sol_amount = get_sell_sol_amount(&client.rpc, &pool_state, amount_token).await?;
    
    // 执行卖出交易
    client.sell(
        DexType::RaydiumCpmm,
        mint_pubkey,
        None,
        amount_token,
        slippage_basis_points,
        recent_blockhash,
        None,
        Some(Box::new(RaydiumCpmmParams {
            pool_state: Some(pool_state),
            mint_token_program: Some(spl_token::ID),
            mint_token_in_pool_state_index: Some(1),
            minimum_amount_out: Some(sell_sol_amount),
            auto_handle_wsol: true,
        })),
    ).await?;
    
    println!("卖出交易完成！");

    // 返回成功结果
    Ok(())
}

/// 通用Raydium CPMM买入函数
/// 
/// # 参数
/// * `sol_amount` - 用于买入的SOL数量（以lamports为单位），如果为0则使用账户全部SOL
/// * `pool_address` - Raydium流动性池地址
/// * `token_address` - 要买入的代币地址
/// * `slippage_basis_points` - 滑点容差（基点），默认100（1%）
/// 
/// # 返回值
/// - `Result<(), Box<dyn std::error::Error>>`: 成功返回`()`，失败返回错误
async fn buy_raydium_cpmm(
    sol_amount: u64,
    pool_address: &str,
    token_address: &str,
    slippage_basis_points: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("开始Raydium CPMM买入交易...");

    // 创建Solana交易客户端
    let client = test_create_solana_trade_client().await?;
    
    // 解析地址
    let mint_pubkey = Pubkey::from_str(token_address)?;
    let pool_state = Pubkey::from_str(pool_address)?;
    
    // 获取账户余额
    let balance = client.rpc.get_balance(&client.get_payer_pubkey()).await?;
    println!("账户SOL余额: {} SOL", balance as f64 / 1_000_000_000.0);
    
    // 确定买入金额
    let buy_sol_cost = if sol_amount == 0 {
        // 如果输入为0，使用账户全部SOL（保留一些作为手续费）
        let available_balance = if balance > 50_000_000 { // 保留0.05 SOL作为手续费
            balance - 50_000_000
        } else {
            balance
        };
        println!("使用全部可用SOL进行买入: {} SOL", available_balance as f64 / 1_000_000_000.0);
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
    
    // 滑点设置
    let slippage = slippage_basis_points.unwrap_or(100);
    
    // 获取最新区块哈希
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;
    
    // 验证池状态是否存在
    let pool_account = client.rpc.get_account(&pool_state).await;
    if pool_account.is_err() {
        return Err(format!("池状态地址无效或池不存在: {}", pool_address).into());
    }
    
    // 计算买入代币的预期数量
    let buy_amount_out = get_buy_token_amount(&client.rpc, &pool_state, buy_sol_cost).await?;
    println!("预期获得代币数量: {}", buy_amount_out);
    
    // 执行买入交易
    println!("正在买入 {} SOL 的 {} 代币...", 
        buy_sol_cost as f64 / 1_000_000_000.0, token_address);
    
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
            println!("买入交易成功完成！");
            println!("买入金额: {} SOL", buy_sol_cost as f64 / 1_000_000_000.0);
            println!("预期获得代币: {}", buy_amount_out);
        }
        Err(e) => {
            println!("买入交易失败: {}", e);
            println!("注意：即使交易失败，手续费也不会退回");
            return Err(e.into());
        }
    }

    Ok(())
}

/// 通用Raydium CPMM卖出函数
/// 
/// # 参数
/// * `token_amount` - 要卖出的代币数量，如果为0则卖出全部持有的代币
/// * `pool_address` - Raydium流动性池地址
/// * `token_address` - 要卖出的代币地址
/// * `slippage_basis_points` - 滑点容差（基点），默认100（1%）
/// 
/// # 返回值
/// - `Result<(), Box<dyn std::error::Error>>`: 成功返回`()`，失败返回错误
async fn sell_raydium_cpmm(
    token_amount: u64,
    pool_address: &str,
    token_address: &str,
    slippage_basis_points: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("开始Raydium CPMM卖出交易...");

    // 创建Solana交易客户端
    let client = test_create_solana_trade_client().await?;
    
    // 解析地址
    let mint_pubkey = Pubkey::from_str(token_address)?;
    let pool_state = Pubkey::from_str(pool_address)?;
    
    // 获取代币余额
    let token_balance = client.get_payer_token_balance(&mint_pubkey).await?;
    println!("账户代币余额: {}", token_balance);
    
    // 确定卖出数量
    let sell_token_amount = if token_amount == 0 {
        // 如果输入为0，卖出全部持有的代币
        println!("卖出全部持有的代币: {}", token_balance);
        token_balance
    } else {
        // 使用指定的代币数量
        if token_amount > token_balance {
            return Err(format!("代币余额不足，需要 {}，当前余额 {}", token_amount, token_balance).into());
        }
        token_amount
    };
    
    if sell_token_amount == 0 {
        return Err("卖出数量不能为0".into());
    }
    
    // 滑点设置
    let slippage = slippage_basis_points.unwrap_or(100);
    
    // 获取最新区块哈希
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;
    
    // 验证池状态是否存在
    let pool_account = client.rpc.get_account(&pool_state).await;
    if pool_account.is_err() {
        return Err(format!("池状态地址无效或池不存在: {}", pool_address).into());
    }
    
    // 计算卖出代币能获得的SOL数量
    let sell_sol_amount = get_sell_sol_amount(&client.rpc, &pool_state, sell_token_amount).await?;
    println!("预期获得SOL数量: {} SOL", sell_sol_amount as f64 / 1_000_000_000.0);
    
    // 执行卖出交易
    println!("正在卖出 {} 个 {} 代币...", sell_token_amount, token_address);
    
    let result = client.sell(
        DexType::RaydiumCpmm,
        mint_pubkey,
        None,
        sell_token_amount,
        Some(slippage),
        recent_blockhash,
        None,
        Some(Box::new(RaydiumCpmmParams {
            pool_state: Some(pool_state),
            mint_token_program: Some(spl_token::ID),
            mint_token_in_pool_state_index: Some(1),
            minimum_amount_out: Some(sell_sol_amount),
            auto_handle_wsol: true,
        })),
    ).await;
    
    match result {
        Ok(_) => {
            println!("卖出交易成功完成！");
            println!("卖出代币数量: {}", sell_token_amount);
            println!("预期获得SOL: {} SOL", sell_sol_amount as f64 / 1_000_000_000.0);
        }
        Err(e) => {
            println!("卖出交易失败: {}", e);
            println!("注意：即使交易失败，手续费也不会退回");
            return Err(e.into());
        }
    }

    Ok(())
}

/// 测试通用买入和卖出函数
/// 
/// 这个函数演示了如何使用新创建的buy_raydium_cpmm和sell_raydium_cpmm函数
async fn test_raydium_cpmm_functions() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== 测试通用Raydium CPMM交易函数 ===");
    
    // BONK代币的配置
    let bonk_token = "6p6xgHyF7AeE6TZkSmFsko444wqoP15icUSqi2jfGiPN";
    let bonk_pool = "HKuJrP5tYQLbEUdjKwjgnHs2957QKjR2iWhJKTtMa1xs";
    
    // 示例1：买入指定数量的SOL
    println!("\n--- 示例1：买入0.01 SOL的BONK代币 ---");
    buy_raydium_cpmm(
        10_000_000, // 0.01 SOL
        bonk_pool,
        bonk_token,
        Some(100), // 1%滑点
    ).await?;
    
    // 示例2：买入全部SOL（除了手续费）
    println!("\n--- 示例2：买入全部SOL的BONK代币 ---");
    buy_raydium_cpmm(
        0, // 0表示买入全部
        bonk_pool,
        bonk_token,
        Some(100), // 1%滑点
    ).await?;
    
    // 示例3：卖出指定数量的代币
    println!("\n--- 示例3：卖出1000个BONK代币 ---");
    sell_raydium_cpmm(
        1000, // 卖出1000个代币
        bonk_pool,
        bonk_token,
        Some(100), // 1%滑点
    ).await?;
    
    // 示例4：卖出全部代币
    println!("\n--- 示例4：卖出全部BONK代币 ---");
    sell_raydium_cpmm(
        0, // 0表示卖出全部
        bonk_pool,
        bonk_token,
        Some(100), // 1%滑点
    ).await?;
    
    println!("=== 测试完成 ===");
    Ok(())
}

/// 测试使用MEV服务的Raydium CPMM交易
/// 
/// 这个函数演示了如何正确使用MEV服务进行交易
/// 注意：使用MEV服务会并行提交多笔交易，需要确保只有一笔成功
async fn test_trade_raydium_cpmm_with_mev() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing Raydium Cpmm trading with MEV services...");

    // 创建Solana交易客户端
    let client = test_create_solana_trade_client().await?;
    
    // 代币的Mint地址
    let mint_pubkey = Pubkey::from_str("6p6xgHyF7AeE6TZkSmFsko444wqoP15icUSqi2jfGiPN")?; // BONK代币
    
    // 买入时使用的SOL数量（以lamports为单位）
    let buy_sol_cost = 10_000_000;  // 0.01 SOL
    
    // 滑点容差设置（以基点为单位）
    let slippage_basis_points = Some(100);
    
    // 获取最新的区块哈希
    let recent_blockhash = client.rpc.get_latest_blockhash().await?;
    
    // Raydium流动性池的状态地址
    let pool_state = Pubkey::from_str("HKuJrP5tYQLbEUdjKwjgnHs2957QKjR2iWhJKTtMa1xs")?; // BONK/SOL池
    
    // 计算买入代币的预期数量
    let buy_amount_out = get_buy_token_amount(&client.rpc, &pool_state, buy_sol_cost).await?;
    
    // ========== 买入操作 ==========
    println!("Buying tokens from Raydium Cpmm with MEV services...");
    
    // 执行买入交易
    client.buy(
        DexType::RaydiumCpmm,
        mint_pubkey,
        None,
        buy_sol_cost,
        slippage_basis_points,
        recent_blockhash,
        None,
        Some(Box::new(RaydiumCpmmParams {
            pool_state: Some(pool_state),
            mint_token_program: Some(spl_token::ID),
            mint_token_in_pool_state_index: Some(1),
            minimum_amount_out: Some(buy_amount_out),
            auto_handle_wsol: true,
        })),
    ).await?;

    println!("买入交易完成！（使用MEV服务，可能有多笔交易提交）");
    
    // 注意：使用MEV服务时，可能会看到多笔交易
    // 这是正常现象，因为SDK会并行提交到多个MEV服务
    // 只有一笔交易会成功执行，其他会被网络拒绝

    Ok(())
}

async fn test_grpc() -> Result<(), Box<dyn std::error::Error>> {
    println!("正在订阅 GRPC 事件...");

    let grpc = YellowstoneGrpc::new(
        "https://solana-yellowstone-grpc.publicnode.com:443".to_string(),
        None,
    )?;

    let callback = create_event_callback();
    let protocols = vec![Protocol::PumpFun, Protocol::PumpSwap, Protocol::Bonk, Protocol::RaydiumCpmm];

    println!("开始监听事件，按 Ctrl+C 停止...");
    grpc.subscribe_events(protocols, None, None, None, None, None, callback).await?;

    Ok(())
}

// async fn test_shreds() -> Result<(), Box<dyn std::error::Error>> {
//     println!("正在订阅 ShredStream 事件...");

//     let shred_stream = ShredStreamGrpc::new("http://127.0.0.1:10800".to_string()).await?;
//     let callback = create_event_callback();
//     let protocols = vec![Protocol::PumpFun, Protocol::PumpSwap, Protocol::Bonk];

//     println!("开始监听事件，按 Ctrl+C 停止...");
//     shred_stream.shredstream_subscribe(protocols, None, callback).await?;

//     Ok(())
// }

/// 测试Raydium Cpmm事件筛选功能
async fn test_raydium_filter() -> Result<(), Box<dyn std::error::Error>> {
    println!("正在测试Raydium Cpmm事件筛选...");

    let grpc = YellowstoneGrpc::new(
        "https://solana-yellowstone-grpc.publicnode.com:443".to_string(),
        None,
    )?;

    let callback = create_raydium_filter_callback();
    let protocols = vec![Protocol::RaydiumCpmm];

    println!("开始监听Raydium Cpmm事件，按 Ctrl+C 停止...");
    grpc.subscribe_events(protocols, None, None, None, None, None, callback).await?;

    Ok(())
}

/// 测试Raydium Cpmm代币筛选功能
/// 
/// # 参数
/// * `target_token` - 目标代币地址
async fn test_raydium_token_filter(target_token: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("正在测试Raydium Cpmm代币筛选，目标代币: {}", target_token);

    let grpc = YellowstoneGrpc::new(
        "https://solana-yellowstone-grpc.publicnode.com:443".to_string(),
        None,
    )?;

    let callback = create_raydium_token_filter_callback(target_token.to_string());
    let protocols = vec![Protocol::RaydiumCpmm];

    println!("开始监听 {} 代币的Raydium Cpmm事件，按 Ctrl+C 停止...", target_token);
    grpc.subscribe_events(protocols, None, None, None, None, None, callback).await?;

    Ok(())
}

/// 测试Raydium Cpmm钱包筛选功能
/// 
/// # 参数
/// * `target_wallet` - 目标钱包地址
async fn test_raydium_wallet_filter(target_wallet: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("正在测试Raydium Cpmm钱包筛选，目标钱包: {}", target_wallet);

    let grpc = YellowstoneGrpc::new(
        "https://solana-yellowstone-grpc.publicnode.com:443".to_string(),
        None,
    )?;

    let callback = create_raydium_wallet_filter_callback(target_wallet.to_string());
    let protocols = vec![Protocol::RaydiumCpmm];

    println!("开始监听 {} 钱包的Raydium Cpmm事件，按 Ctrl+C 停止...", target_wallet);
    grpc.subscribe_events(protocols, None, None, None, None, None, callback).await?;

    Ok(())
}

/// 创建事件回调函数
/// 
/// 这个函数返回一个闭包，用于处理来自不同 DEX 协议的各种事件。
/// 当 Solana 区块链上发生交易、池创建、代币创建等事件时，
/// 这个回调函数会被调用来处理相应的事件。
/// 
/// # 功能说明
/// 
/// - **Bonk 协议事件**: 处理 Bonk 代币的池创建和交易事件
/// - **PumpFun 协议事件**: 处理 PumpFun 的交易和代币创建事件  
/// - **PumpSwap 协议事件**: 处理 PumpSwap 的买卖、池创建、存款和提款事件
/// - **Raydium CPMM 事件**: 处理 Raydium 恒定乘积做市商协议的交换事件
/// 
/// # 返回值
/// 
/// 返回一个闭包函数，接受 `Box<dyn UnifiedEvent>` 类型的事件参数
/// 
/// # 使用示例
/// 
/// ```rust
/// let callback = create_event_callback();
/// // 在事件流订阅中使用这个回调
/// ```
fn create_event_callback() -> impl Fn(Box<dyn UnifiedEvent>) {
    |event: Box<dyn UnifiedEvent>| {
        // 使用标准 Rust 模式匹配来处理不同类型的事件
        if let Some(raydium_event) = event.as_any().downcast_ref::<RaydiumCpmmSwapEvent>() {
            println!("RaydiumCpmmSwapEvent: {:?}", raydium_event);
        }
        // 可以在这里添加其他协议的事件处理
    }
}

/// 创建Raydium事件筛选回调函数
fn create_raydium_filter_callback() -> impl Fn(Box<dyn UnifiedEvent>) {
    |event: Box<dyn UnifiedEvent>| {
        if let Some(raydium_event) = event.as_any().downcast_ref::<RaydiumCpmmSwapEvent>() {
            process_raydium_event(raydium_event);
        }
    }
}

/// 创建Raydium代币筛选回调函数
/// 
/// # 参数
/// * `target_token` - 目标代币地址
fn create_raydium_token_filter_callback(target_token: String) -> impl Fn(Box<dyn UnifiedEvent>) {
    move |event: Box<dyn UnifiedEvent>| {
        if let Some(raydium_event) = event.as_any().downcast_ref::<RaydiumCpmmSwapEvent>() {
            process_raydium_event_by_token(raydium_event, &target_token);
        }
    }
}

/// 创建Raydium钱包筛选回调函数
/// 
/// # 参数
/// * `target_wallet` - 目标钱包地址
fn create_raydium_wallet_filter_callback(target_wallet: String) -> impl Fn(Box<dyn UnifiedEvent>) {
    move |event: Box<dyn UnifiedEvent>| {
        if let Some(raydium_event) = event.as_any().downcast_ref::<RaydiumCpmmSwapEvent>() {
            process_raydium_event_by_wallet(raydium_event, &target_wallet);
        }
    }
}

/// 处理Raydium Cpmm交换事件，筛选指定钱包地址的交易
fn process_raydium_event(event: &RaydiumCpmmSwapEvent) {
    let target_wallet = "72Wnk8BcBawFduXsugd3f7LwSMBmGB1JzFQuJSLyDb2N";
    
    // 检查transfer_datas中是否包含目标钱包地址
    let mut found_transaction = false;
    let mut input_token = None;
    let mut output_token = None;
    let mut input_amount = 0u64;
    let mut output_amount = 0u64;
    let mut input_decimals = 0u8;
    let mut output_decimals = 0u8;
    
    for transfer in &event.metadata.transfer_datas {
        let source = transfer.source.to_string();
        let destination = transfer.destination.to_string();
        
        // 检查是否是目标钱包的交易
        if source == target_wallet || destination == target_wallet {
            found_transaction = true;
            
            if let Some(mint) = &transfer.mint {
                let mint_str = mint.to_string();
                let amount = transfer.amount;
                let decimals = transfer.decimals.unwrap_or(0);
                
                if source == target_wallet {
                    // 钱包是源地址，表示卖出
                    output_token = Some(mint_str.clone());
                    output_amount = amount;
                    output_decimals = decimals;
                    println!("🔴 卖出: {} {} (精度: {})", 
                        format_amount(amount, decimals), 
                        get_token_symbol(&mint_str), 
                        decimals);
                } else {
                    // 钱包是目标地址，表示买入
                    input_token = Some(mint_str.clone());
                    input_amount = amount;
                    input_decimals = decimals;
                    println!("🟢 买入: {} {} (精度: {})", 
                        format_amount(amount, decimals), 
                        get_token_symbol(&mint_str), 
                        decimals);
                }
            }
        }
    }
    
    if found_transaction {
        println!("=== Raydium Cpmm 交易详情 ===");
        println!("交易ID: {}", event.metadata.id);
        println!("签名: {}", event.metadata.signature);
        println!("时间戳: {}", event.metadata.block_time);
        println!("输入Token: {:?}", input_token);
        println!("输出Token: {:?}", output_token);
        println!("输入金额: {}", format_amount(input_amount, input_decimals));
        println!("输出金额: {}", format_amount(output_amount, output_decimals));
        println!("支付者: {}", event.payer);
        println!("权限: {}", event.authority);
        println!("池状态: {}", event.pool_state);
        println!("输入Token账户: {}", event.input_token_account);
        println!("输出Token账户: {}", event.output_token_account);
        println!("输入Token Mint: {}", event.input_token_mint);
        println!("输出Token Mint: {}", event.output_token_mint);
        println!("================================");
    }
}

/// 处理Raydium Cpmm交换事件，筛选指定代币的交易
/// 
/// # 参数
/// * `event` - Raydium CPMM交换事件
/// * `target_token` - 目标代币地址
fn process_raydium_event_by_token(event: &RaydiumCpmmSwapEvent, target_token: &str) {
    // 检查transfer_datas中是否包含目标代币
    let mut found_transaction = false;
    let mut input_token = None;
    let mut output_token = None;
    let mut input_amount = 0u64;
    let mut output_amount = 0u64;
    let mut input_decimals = 0u8;
    let mut output_decimals = 0u8;
    let mut wallet_address = None;
    
    for transfer in &event.metadata.transfer_datas {
        if let Some(mint) = &transfer.mint {
            let mint_str = mint.to_string();
            
            // 检查是否是目标代币的交易
            if mint_str == target_token {
                found_transaction = true;
                let amount = transfer.amount;
                let decimals = transfer.decimals.unwrap_or(0);
                let source = transfer.source.to_string();
                let destination = transfer.destination.to_string();
                
                // 记录钱包地址（用于显示交易方向）
                if wallet_address.is_none() {
                    wallet_address = Some(source.clone());
                }
                
                // 判断是买入还是卖出
                if source == event.payer.to_string() {
                    // 支付者是源地址，表示买入
                    input_token = Some(mint_str.clone());
                    input_amount = amount;
                    input_decimals = decimals;
                    println!("🟢 买入: {} {} (精度: {})", 
                        format_amount(amount, decimals), 
                        get_token_symbol(&mint_str), 
                        decimals);
                } else {
                    // 支付者是目标地址，表示卖出
                    output_token = Some(mint_str.clone());
                    output_amount = amount;
                    output_decimals = decimals;
                    println!("🔴 卖出: {} {} (精度: {})", 
                        format_amount(amount, decimals), 
                        get_token_symbol(&mint_str), 
                        decimals);
                }
            }
        }
    }
    
    if found_transaction {
        println!("=== Raydium Cpmm 代币交易详情 ===");
        println!("目标代币: {}", target_token);
        println!("交易ID: {}", event.metadata.id);
        println!("签名: {}", event.metadata.signature);
        println!("时间戳: {}", event.metadata.block_time);
        println!("钱包地址: {}", wallet_address.unwrap_or_else(|| "未知".to_string()));
        println!("输入Token: {:?}", input_token);
        println!("输出Token: {:?}", output_token);
        println!("输入金额: {}", format_amount(input_amount, input_decimals));
        println!("输出金额: {}", format_amount(output_amount, output_decimals));
        println!("支付者: {}", event.payer);
        println!("权限: {}", event.authority);
        println!("池状态: {}", event.pool_state);
        println!("输入Token账户: {}", event.input_token_account);
        println!("输出Token账户: {}", event.output_token_account);
        println!("输入Token Mint: {}", event.input_token_mint);
        println!("输出Token Mint: {}", event.output_token_mint);
        println!("================================");
    }
}

/// 处理Raydium Cpmm交换事件，筛选指定钱包地址的交易
/// 
/// # 参数
/// * `event` - Raydium CPMM交换事件
/// * `target_wallet` - 目标钱包地址
fn process_raydium_event_by_wallet(event: &RaydiumCpmmSwapEvent, target_wallet: &str) {
    // 检查transfer_datas中是否包含目标钱包地址
    let mut found_transaction = false;
    let mut input_token = None;
    let mut output_token = None;
    let mut input_amount = 0u64;
    let mut output_amount = 0u64;
    let mut input_decimals = 0u8;
    let mut output_decimals = 0u8;
    
    for transfer in &event.metadata.transfer_datas {
        let source = transfer.source.to_string();
        let destination = transfer.destination.to_string();
        
        // 检查是否是目标钱包的交易
        if source == target_wallet || destination == target_wallet {
            found_transaction = true;
            
            if let Some(mint) = &transfer.mint {
                let mint_str = mint.to_string();
                let amount = transfer.amount;
                let decimals = transfer.decimals.unwrap_or(0);
                
                if source == target_wallet {
                    // 钱包是源地址，表示卖出
                    output_token = Some(mint_str.clone());
                    output_amount = amount;
                    output_decimals = decimals;
                    println!("🔴 卖出: {} {} (精度: {})", 
                        format_amount(amount, decimals), 
                        get_token_symbol(&mint_str), 
                        decimals);
                } else {
                    // 钱包是目标地址，表示买入
                    input_token = Some(mint_str.clone());
                    input_amount = amount;
                    input_decimals = decimals;
                    println!("🟢 买入: {} {} (精度: {})", 
                        format_amount(amount, decimals), 
                        get_token_symbol(&mint_str), 
                        decimals);
                }
            }
        }
    }
    
    if found_transaction {
        println!("=== Raydium Cpmm 钱包交易详情 ===");
        println!("目标钱包: {}", target_wallet);
        println!("交易ID: {}", event.metadata.id);
        println!("签名: {}", event.metadata.signature);
        println!("时间戳: {}", event.metadata.block_time);
        println!("输入Token: {:?}", input_token);
        println!("输出Token: {:?}", output_token);
        println!("输入金额: {}", format_amount(input_amount, input_decimals));
        println!("输出金额: {}", format_amount(output_amount, output_decimals));
        println!("支付者: {}", event.payer);
        println!("权限: {}", event.authority);
        println!("池状态: {}", event.pool_state);
        println!("输入Token账户: {}", event.input_token_account);
        println!("输出Token账户: {}", event.output_token_account);
        println!("输入Token Mint: {}", event.input_token_mint);
        println!("输出Token Mint: {}", event.output_token_mint);
        println!("================================");
    }
}

/// 格式化金额，处理精度
fn format_amount(amount: u64, decimals: u8) -> String {
    if decimals == 0 {
        return amount.to_string();
    }
    
    let divisor = 10u64.pow(decimals as u32);
    let integer_part = amount / divisor;
    let fractional_part = amount % divisor;
    
    if fractional_part == 0 {
        integer_part.to_string()
    } else {
        let fractional_str = format!("{:0width$}", fractional_part, width = decimals as usize);
        format!("{}.{}", integer_part, fractional_str.trim_end_matches('0'))
    }
}

/// 获取Token符号（简化版本）
fn get_token_symbol(mint: &str) -> String {
    match mint {
        "So11111111111111111111111111111111111111112" => "SOL".to_string(),
        "76HyhyHuHdm1VuvLSfBqorfCZN8sNRFHjccMdRksbonk" => "BONK".to_string(),
        _ => {
            if mint.len() > 8 {
                format!("{}...", &mint[..8])
            } else {
                mint.to_string()
            }
        }
    }
}
