use chrono::{DateTime, Utc};
use log::{Level, LevelFilter, Log, Metadata, Record};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

/// 日志记录器结构体
pub struct Logger {
    level: LevelFilter,
    file: Option<Arc<Mutex<File>>>,
    console_output: bool,
    log_file_path: Option<String>,
    max_file_size: u64, // 最大文件大小（字节）
}

impl Logger {
    /// 创建新的日志记录器
    pub fn new(level: LevelFilter, log_file_path: Option<&str>, console_output: bool) -> io::Result<Self> {
        let file = if let Some(path) = log_file_path {
            // 确保日志目录存在
            if let Some(parent) = Path::new(path).parent() {
                std::fs::create_dir_all(parent)?;
            }
            
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            Some(Arc::new(Mutex::new(file)))
        } else {
            None
        };

        Ok(Self {
            level,
            file,
            console_output,
            log_file_path: log_file_path.map(|s| s.to_string()),
            max_file_size: 10 * 1024 * 1024, // 10MB
        })
    }

    /// 初始化日志系统
    pub fn init(level: LevelFilter, log_file_path: Option<&str>, console_output: bool) -> io::Result<()> {
        let logger = Self::new(level, log_file_path, console_output)?;
        log::set_max_level(level);
        log::set_boxed_logger(Box::new(logger))?;
        Ok(())
    }

    /// 检查并轮转日志文件
    fn check_and_rotate_log_file(&self) -> io::Result<()> {
        if let (Some(file), Some(path)) = (&self.file, &self.log_file_path) {
            let file_size = {
                let file = file.lock().unwrap();
                file.metadata()?.len()
            };

            if file_size > self.max_file_size {
                self.rotate_log_file(path)?;
            }
        }
        Ok(())
    }

    /// 轮转日志文件
    fn rotate_log_file(&self, current_path: &str) -> io::Result<()> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let backup_path = format!("{}.{}", current_path, timestamp);
        
        // 重命名当前日志文件
        if Path::new(current_path).exists() {
            std::fs::rename(current_path, &backup_path)?;
        }

        // 创建新的日志文件
        let new_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(current_path)?;
        
        // 更新文件句柄
        if let Some(file) = &self.file {
            let mut file_guard = file.lock().unwrap();
            *file_guard = new_file;
        }

        Ok(())
    }

    /// 写入日志到文件
    fn write_to_file(&self, record: &Record) -> io::Result<()> {
        // 检查是否需要轮转日志文件
        self.check_and_rotate_log_file()?;

        if let Some(file) = &self.file {
            let mut file = file.lock().unwrap();
            let timestamp: DateTime<Utc> = Utc::now();
            let log_entry = format!(
                "[{}] [{}] [{}:{}:{}] [{}] {}\n",
                timestamp.format("%Y-%m-%d %H:%M:%S%.3f"),
                record.level(),
                record.target(),
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                std::thread::current().name().unwrap_or("main"),
                record.args()
            );
            file.write_all(log_entry.as_bytes())?;
            file.flush()?;
        }
        Ok(())
    }

    /// 写入日志到控制台
    fn write_to_console(&self, record: &Record) {
        if self.console_output {
            let timestamp: DateTime<Utc> = Utc::now();
            let log_entry = format!(
                "[{}] [{}] [{}:{}:{}] [{}] {}\n",
                timestamp.format("%H:%M:%S%.3f"),
                record.level(),
                record.target(),
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                std::thread::current().name().unwrap_or("main"),
                record.args()
            );
            
            // 根据日志级别使用不同的颜色
            let colored_output = match record.level() {
                Level::Error => format!("\x1b[31m{}\x1b[0m", log_entry), // 红色
                Level::Warn => format!("\x1b[33m{}\x1b[0m", log_entry),  // 黄色
                Level::Info => format!("\x1b[36m{}\x1b[0m", log_entry),  // 青色
                Level::Debug => format!("\x1b[35m{}\x1b[0m", log_entry), // 紫色
                Level::Trace => log_entry,
            };
            
            print!("{}", colored_output);
        }
    }

    /// 设置最大日志文件大小
    pub fn set_max_file_size(&mut self, size_bytes: u64) {
        self.max_file_size = size_bytes;
    }

    /// 手动轮转日志文件
    pub fn rotate_log(&self) -> io::Result<()> {
        if let Some(path) = &self.log_file_path {
            self.rotate_log_file(path)
        } else {
            Ok(())
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // 写入文件
            if let Err(e) = self.write_to_file(record) {
                eprintln!("写入日志文件失败: {}", e);
            }
            
            // 写入控制台
            self.write_to_console(record);
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            if let Ok(mut file) = file.lock() {
                let _ = file.flush();
            }
        }
    }
}

/// 全局日志记录器实例
lazy_static::lazy_static! {
    static ref GLOBAL_LOGGER: Arc<Mutex<Option<Logger>>> = Arc::new(Mutex::new(None));
}

/// 获取全局日志记录器
pub fn get_global_logger() -> Option<Logger> {
    GLOBAL_LOGGER.lock().unwrap().clone()
}

/// 设置全局日志记录器
pub fn set_global_logger(logger: Logger) {
    let mut global = GLOBAL_LOGGER.lock().unwrap();
    *global = Some(logger);
}

/// 手动轮转全局日志文件
pub fn rotate_global_log() -> io::Result<()> {
    if let Some(logger) = get_global_logger() {
        logger.rotate_log()
    } else {
        Ok(())
    }
}

/// 便捷的日志宏
#[macro_export]
macro_rules! log_trade {
    ($level:expr, $($arg:tt)*) => {
        log::log!($level, $($arg)*);
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        log::info!($($arg)*);
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        log::warn!($($arg)*);
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        log::error!($($arg)*);
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        log::debug!($($arg)*);
    };
}
