# Solana 交易策略服务

这是一个基于 Rust 开发的 Solana 交易策略服务，支持多种交易策略，特别针对 Raydium CPMM 池的止盈止损策略。

## 功能特性

### 🎯 核心功能
- **钱包监控**: 监控指定钱包的交易活动
- **跟单交易**: 自动跟随监控钱包的买入操作
- **止盈止损**: 基于配置的止盈止损策略自动卖出
- **价格监控**: 实时监控 Token 价格变化
- **持仓管理**: 自动跟踪和管理持仓信息

### 🔧 技术特性
- **异步架构**: 基于 Tokio 的高性能异步运行时
- **日志系统**: 完整的日志记录和文件轮转
- **配置管理**: 安全的配置文件管理
- **错误处理**: 完善的错误处理和恢复机制
- **监控统计**: 实时策略执行统计和状态监控

## 快速开始

### 1. 环境要求
- Rust 1.70+ 
- Solana CLI 工具
- 有效的 Solana 钱包和私钥

### 2. 安装依赖
```bash
# 克隆项目
git clone https://github.com/your-repo/sol-trade-sdk.git
cd sol-trade-sdk

# 安装依赖
cargo build --release
```

### 3. 配置设置
复制配置文件模板并修改：
```bash
cp config.toml.example config.toml
```

编辑 `config.toml` 文件，设置以下关键配置：

```toml
[authentication]
private_key = "YOUR_PRIVATE_KEY_HERE"  # 您的私钥

[wallet_monitoring]
wallet_addresses = [
    "WALLET_ADDRESS_1_HERE",  # 要监控的钱包地址
    "WALLET_ADDRESS_2_HERE"
]

[take_profit_stop_loss]
take_profit_percentage = 0.20  # 20% 止盈
stop_loss_percentage = 0.10    # 10% 止损
max_buy_amount_sol = 5000000000  # 5 SOL 最大买入
min_buy_amount_sol = 100000000   # 0.1 SOL 最小买入
```

### 4. 启动服务
```bash
# 启动服务
cargo run --release

# 或者直接运行编译后的二进制文件
./target/release/sol-trade-sdk
```

## 配置说明

### 网络配置
```toml
[network]
network_type = "mainnet-beta"  # mainnet-beta, testnet, devnet
```

### RPC 配置
```toml
[rpc]
main_rpc_url = "https://api.mainnet-beta.solana.com"
strategy_rpc_url = "https://api.mainnet-beta.solana.com"  # 策略执行专用RPC
```

### 交易配置
```toml
[trading]
default_slippage_basis_points = 100  # 1% 滑点
default_buy_amount_sol = 1000000000  # 1 SOL 默认买入金额
auto_handle_wsol = true              # 自动处理 wSOL
```

### 优先费用配置
```toml
[priority_fees]
compute_unit_limit = 200000          # 计算单元限制
compute_unit_price = 1000            # 计算单元价格
buy_tip_fee = 0.001                 # 买入小费 0.1%
sell_tip_fee = 0.0005               # 卖出小费 0.05%
```

### 日志配置
```toml
[logging]
log_level = "INFO"                   # 日志级别
log_to_file = true                   # 启用文件日志
log_file_path = "logs/trading_strategy.log"  # 日志文件路径
```

## 策略工作流程

### 1. 钱包监控阶段
- 监控指定钱包的交易活动
- 检测买入和卖出事件
- 记录交易详情和签名

### 2. 跟单买入阶段
- 当监控钱包执行买入时，自动触发跟单
- 按照配置的比例和金额限制执行买入
- 更新持仓信息和买入成本

### 3. 价格监控阶段
- 实时监控持仓 Token 的价格变化
- 计算相对于买入价格的盈亏百分比
- 触发止盈止损检查

### 4. 止盈止损执行阶段
- 达到止盈条件时自动卖出
- 达到止损条件时自动卖出
- 更新持仓状态和交易记录

## 安全注意事项

### 🔒 私钥安全
- **永远不要**将私钥提交到版本控制系统
- 使用环境变量或安全的密钥管理服务
- 定期轮换私钥

### 📁 配置文件安全
- 确保 `config.toml` 在 `.gitignore` 中
- 不要在生产环境中使用示例配置
- 定期审查和更新配置

### 🌐 网络安全
- 使用可信的 RPC 端点
- 启用 MEV 保护服务
- 监控网络连接状态

## 监控和日志

### 日志文件
服务会生成详细的日志文件，包含：
- 交易执行记录
- 策略触发事件
- 错误和警告信息
- 性能统计信息

### 日志轮转
- 自动日志文件轮转（默认 10MB）
- 时间戳命名备份文件
- 可配置的最大文件大小

### 监控指标
- 监控钱包数量
- 当前持仓数量
- 策略执行成功率
- 总盈亏统计

## 故障排除

### 常见问题

#### 1. 私钥验证失败
```
错误: 私钥格式错误
解决: 检查私钥是否为有效的 base58 编码
```

#### 2. RPC 连接失败
```
错误: 无法连接到 RPC 端点
解决: 检查网络连接和 RPC URL 配置
```

#### 3. 交易执行失败
```
错误: 交易执行失败
解决: 检查余额、滑点设置和网络拥堵情况
```

#### 4. 日志文件权限错误
```
错误: 无法写入日志文件
解决: 检查日志目录权限和磁盘空间
```

### 调试模式
启用调试日志：
```toml
[logging]
log_level = "DEBUG"
```

### 性能优化
- 调整 RPC 连接池大小
- 优化监控间隔时间
- 使用专用 RPC 端点

## 开发指南

### 项目结构
```
src/
├── common/           # 通用模块
│   ├── config.rs     # 配置管理
│   ├── logger.rs     # 日志系统
│   └── types.rs      # 类型定义
├── strategy/         # 策略模块
│   ├── wallet_monitor.rs      # 钱包监控
│   ├── price_monitor.rs       # 价格监控
│   └── take_profit_stop_loss.rs # 止盈止损策略
├── trading/          # 交易模块
│   └── raydium_cpmm/ # Raydium CPMM 交易
└── service.rs        # 主服务逻辑
```

### 添加新策略
1. 在 `src/strategy/` 目录下创建新策略文件
2. 实现策略接口和逻辑
3. 在 `src/strategy/mod.rs` 中导出新模块
4. 在 `src/service.rs` 中集成新策略

### 测试策略
```bash
# 运行单元测试
cargo test

# 运行集成测试
cargo test --test integration_tests

# 运行特定测试
cargo test test_wallet_monitoring
```

## 许可证

本项目采用 MIT 许可证。详见 [LICENSE](LICENSE) 文件。

## 贡献

欢迎提交 Issue 和 Pull Request！

### 贡献指南
1. Fork 项目
2. 创建功能分支
3. 提交更改
4. 推送到分支
5. 创建 Pull Request

## 支持

如果您需要帮助或有建议，请：
- 提交 GitHub Issue
- 查看项目文档
- 联系维护团队

---

**免责声明**: 本软件仅供学习和研究使用。交易加密货币存在风险，请谨慎投资，风险自负。
