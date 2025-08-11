use std::sync::Arc;
use sol_trade_sdk::{
    common::{AppConfig, log_info, log_warn, log_error},
    service::TradingStrategyService,
};
use anyhow::Result;
use tokio::signal;

/// 主程序入口 - 交易策略服务启动入口
/// 
/// 程序流程：
/// 1. 从config.toml加载配置文件
/// 2. 创建交易策略服务实例
/// 3. 启动服务，开始执行交易策略
/// 4. 等待Ctrl+C信号，优雅关闭服务
/// 
/// 配置文件说明：
/// - 所有配置从config.toml读取，确保安全性
/// - 私钥等敏感信息不会上传到代码仓库
/// - 支持多种配置选项，满足不同需求
#[tokio::main]
async fn main() -> Result<()> {
    // 加载配置文件 - 从config.toml读取所有配置信息
    // 包括RPC端点、私钥、策略参数、监控钱包等
    // 配置文件不上传到代码仓库，确保私钥等敏感信息的安全
    let config = match AppConfig::load_from_file("config.toml") {
        Ok(config) => {
            log_info!("配置文件加载成功");
            config
        }
        Err(e) => {
            eprintln!("加载配置文件失败: {}", e);
            return Err(e);
        }
    };

    // 创建交易策略服务 - 初始化所有策略模块和监控系统
    // 包括钱包监控、价格监控、止盈止损策略等
    // 服务创建成功后，所有模块准备就绪，可以开始执行策略
    let service = match TradingStrategyService::new(config).await {
        Ok(service) => {
            log_info!("交易策略服务创建成功");
            service
        }
        Err(e) => {
            log_error!("创建交易策略服务失败: {}", e);
            return Err(e);
        }
    };

    // 启动服务 - 开始执行交易策略循环
    // 启动后会自动执行：钱包监控 -> 跟单买入 -> 持仓管理 -> 价格监控 -> 止盈止损
    // 所有操作都会记录到日志文件，便于监控和调试
    if let Err(e) = service.start().await {
        log_error!("启动服务失败: {}", e);
        return Err(e);
    }

    log_info!("交易策略服务已启动，按 Ctrl+C 停止...");

    // 等待中断信号 - 程序运行直到收到Ctrl+C信号
    // 这样可以优雅地关闭服务，确保所有交易和监控任务正确停止
    // 避免数据丢失和交易状态不一致的问题
    if let Err(e) = signal::ctrl_c().await {
        log_error!("等待中断信号失败: {}", e);
    }

    log_info!("收到停止信号，正在关闭服务...");

    // 停止服务 - 优雅关闭所有监控任务和策略执行
    // 确保所有进行中的交易完成，清理资源，保存状态
    // 这是程序退出的重要步骤，避免数据丢失
    if let Err(e) = service.stop().await {
        log_error!("停止服务失败: {}", e);
    }

    log_info!("服务已停止");
    Ok(())
}
