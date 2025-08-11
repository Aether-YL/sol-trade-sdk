# DEX监听功能说明

## 概述

本项目已成功添加了DEX（去中心化交易所）监听功能，能够实时监控多个DEX的交易活动并解析出价格信息。这个功能扩展了原有的钱包监控系统，使其能够从DEX交易数据中获取实时价格，为交易策略提供更准确的市场数据。

## 支持的DEX

项目目前支持以下DEX的监听：

1. **Raydium CPMM** - Raydium的集中流动性做市商协议
2. **PumpFun** - PumpFun交易协议
3. **PumpSwap** - PumpSwap交易协议
4. **Bonk** - Bonk交易协议

## 功能特性

### 1. 实时交易监听
- 监控指定DEX的所有交易活动
- 自动解析交易数据，提取价格信息
- 支持买入和卖出交易的识别

### 2. 价格解析和缓存
- 从交易数据中计算实时价格
- 维护价格缓存，避免重复计算
- 支持价格历史记录查询

### 3. 配置化管理
- 通过`config.toml`配置要监听的DEX类型
- 可配置监听间隔和缓存TTL
- 支持动态启用/禁用DEX监听

### 4. 数据管理
- 自动清理过期的交易记录和价格缓存
- 限制内存使用，防止数据积累过多
- 提供完整的查询接口

## 配置说明

在`config.toml`中添加以下配置：

```toml
[wallet_monitoring]
# 钱包监控配置
enabled = true
wallet_addresses = [
    "YOUR_WALLET_ADDRESS_1",
    "YOUR_WALLET_ADDRESS_2"
]
min_buy_amount_sol = 10000

# DEX交易监听配置
enable_dex_monitoring = true  # 启用DEX交易监听
monitored_dex_types = [       # 要监听的DEX类型
    "raydiumcpmm",            # Raydium CPMM
    "pumpfun",                # PumpFun
    "pumpswap",               # PumpSwap
    "bonk"                    # Bonk
]
dex_monitoring_interval_seconds = 5  # DEX监听间隔：5秒
price_cache_ttl_seconds = 300        # 价格缓存TTL：5分钟
```

## 核心组件

### 1. DexTransaction结构体
```rust
pub struct DexTransaction {
    pub dex_type: DexType,                    // DEX类型
    pub token_mint: String,                   // Token的Mint地址
    pub transaction_type: TransactionType,    // 交易类型（买入/卖出）
    pub amount_sol: u64,                      // SOL数量
    pub amount_token: u64,                    // Token数量
    pub timestamp: DateTime<Utc>,             // 时间戳
    pub signature: String,                    // 交易签名
    pub pool_address: Option<String>,         // 池地址
    pub price_sol_per_token: f64,            // 价格（SOL/Token）
}
```

### 2. TokenPrice结构体
```rust
pub struct TokenPrice {
    pub token_mint: String,                   // Token的Mint地址
    pub price_sol: f64,                       // SOL价格
    pub price_usd: Option<f64>,               // USD价格（可选）
    pub volume_24h: Option<u64>,              // 24小时交易量（可选）
    pub timestamp: DateTime<Utc>,             // 时间戳
    pub source: String,                       // 价格来源（DEX名称）
}
```

### 3. PriceCache结构体
```rust
pub struct PriceCache {
    pub prices: HashMap<String, TokenPrice>,  // 价格映射
    pub last_update: DateTime<Utc>,           // 最后更新时间
}
```

## API接口

### 1. 启动/停止DEX监听
```rust
// 启动DEX监听
wallet_monitor.start_dex_monitoring().await?;

// 停止DEX监听
wallet_monitor.stop_dex_monitoring().await?;
```

### 2. 查询DEX交易记录
```rust
// 获取所有DEX交易记录
let transactions = wallet_monitor.get_dex_transactions().await;

// 获取特定DEX的交易记录
let raydium_txs = wallet_monitor.get_dex_transactions_by_type(&DexType::RaydiumCpmm).await;

// 获取特定Token的交易记录
let token_txs = wallet_monitor.get_dex_transactions_by_token("TOKEN_MINT_ADDRESS").await;
```

### 3. 查询价格信息
```rust
// 获取特定Token的当前价格
let price = wallet_monitor.get_token_price("TOKEN_MINT_ADDRESS").await;

// 获取所有Token价格
let all_prices = wallet_monitor.get_all_token_prices().await;
```

### 4. 数据清理
```rust
// 清理过期的价格缓存
wallet_monitor.cleanup_expired_price_cache().await?;

// 清理过期的DEX交易记录
wallet_monitor.cleanup_expired_dex_transactions().await?;
```

## 服务集成

DEX监听功能已完全集成到主服务中：

### 1. 自动启动
- 服务启动时自动启动DEX监听任务（如果启用）
- 根据配置的间隔时间定期执行监听

### 2. 状态监控
- 在策略状态日志中显示DEX监听状态
- 包含交易记录数量和价格缓存数量

### 3. 数据清理
- 定期清理过期的DEX数据
- 维护系统性能

## 实现细节

### 1. 监听循环
```rust
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

    loop {
        interval_timer.tick().await;
        if let Err(e) = Self::monitor_dex_transactions(...).await {
            log_error!("DEX监听错误: {}", e);
        }
    }
}
```

### 2. DEX特定监听
每个DEX都有专门的监听方法：
- `monitor_raydium_cpmm()` - 监听Raydium CPMM
- `monitor_pumpfun()` - 监听PumpFun
- `monitor_pumpswap()` - 监听PumpSwap
- `monitor_bonk()` - 监听Bonk

### 3. 价格更新
```rust
async fn update_price_cache(
    price_cache: &Arc<RwLock<PriceCache>>,
    token_mint: &str,
    price_sol: f64,
    source: &str,
) {
    let mut prices = price_cache.write().await;
    let now = Utc::now();
    
    if let Some(token_price) = prices.prices.get_mut(token_mint) {
        // 更新现有价格
        token_price.price_sol = price_sol;
        token_price.source = source.to_string();
        token_price.timestamp = now;
    } else {
        // 添加新价格
        let new_price = TokenPrice { ... };
        prices.prices.insert(token_mint.to_string(), new_price);
    }
}
```

## 使用场景

### 1. 实时价格监控
- 监控特定Token在不同DEX上的价格
- 识别价格差异，发现套利机会

### 2. 市场分析
- 分析交易量趋势
- 监控大额交易活动

### 3. 策略优化
- 基于实时价格调整交易策略
- 优化止盈止损点位

## 注意事项

### 1. 性能考虑
- 监听间隔不宜过短，避免RPC调用过于频繁
- 定期清理过期数据，防止内存泄漏

### 2. 错误处理
- 监听过程中出现错误会记录日志，但不会中断服务
- 建议监控日志，及时发现和处理问题

### 3. 配置验证
- 启动时会验证DEX类型配置的有效性
- 不支持的DEX类型会被记录警告日志

## 扩展性

该架构设计具有良好的扩展性：

### 1. 添加新DEX
- 在`DexType`枚举中添加新的DEX类型
- 实现对应的监听方法
- 在工厂方法中添加支持

### 2. 增强价格解析
- 可以添加更多价格来源（如API）
- 支持更复杂的价格计算逻辑
- 添加价格验证和过滤机制

### 3. 数据存储
- 可以扩展为数据库存储
- 支持历史数据查询和分析
- 添加数据导出功能

## 总结

DEX监听功能的添加大大增强了项目的市场监控能力，使其能够：

1. **实时获取价格数据** - 从多个DEX的交易中解析出准确的价格信息
2. **支持多种DEX** - 覆盖Solana生态中的主要交易协议
3. **配置化管理** - 灵活配置监听的DEX类型和参数
4. **完整的数据管理** - 提供查询、清理等完整的数据管理功能
5. **服务集成** - 完全集成到主服务中，自动启动和监控

这个功能为交易策略提供了更准确、更及时的市场数据，是项目功能的重要扩展。
