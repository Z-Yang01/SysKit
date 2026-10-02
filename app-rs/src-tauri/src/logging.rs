//! 最小文件日志器：`log` 宏 → `%LOCALAPPDATA%\SysKit\logs\`。
//!
//! Python 版 loguru 双日志（syskit.log 正常 / error.log 错误，7 天清理）的
//! 等价最小实现——真机冒烟驱动 LogWatcher 与前端日志页的数据源。
//! 每条开-写-关（低频写入下简单可靠；句柄常开对外部轮转/清理不友好）。

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::paths;
use crate::timefmt;

/// 与 Python `logger.py` 的 LOG_RETENTION_DAYS 对齐。
const RETENTION_DAYS: u64 = 7;

struct FileLogger;

fn write_line(name: &str, line: &str) {
    let dir = paths::log_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path: PathBuf = dir.join(name);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "[{}] [{}] [{}] {}\n",
            timefmt::now_fmt("%Y-%m-%d %H:%M:%S"),
            record.level(),
            record.target(),
            record.args()
        );
        write_line("syskit.log", &line);
        if record.level() == log::Level::Error {
            write_line("error.log", &line);
        }
    }

    fn flush(&self) {}
}

/// 安装全局 logger（重复调用幂等：全局 logger 进程内只能装一次）。
pub fn init() {
    let _ = log::set_boxed_logger(Box::new(FileLogger));
    log::set_max_level(log::LevelFilter::Info);
    prune_old();
}

/// 清理超过保留期的 .log 文件（启动时机，与 Python 侧同口径）。
fn prune_old() {
    let dir = paths::log_dir();
    let Ok(rd) = fs::read_dir(&dir) else {
        return;
    };
    let cutoff =
        SystemTime::now().checked_sub(std::time::Duration::from_secs(RETENTION_DAYS * 86400));
    let Some(cutoff) = cutoff else { return };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_file() || !p.extension().is_some_and(|x| x == "log") {
            continue;
        }
        if let Ok(meta) = e.metadata() {
            if meta.modified().map(|m| m < cutoff).unwrap_or(false) {
                let _ = fs::remove_file(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_lines_to_syskit_log() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate(); // 钉住数据目录（日志路径随 override）
        init();
        log::info!("冒烟日志写入探针 smoke-probe-marker");
        log::error!("错误分流探针 smoke-err-marker");
        let dir = paths::log_dir(); // = <隔离根>/local/logs（override 生效）
        let syskit = std::fs::read_to_string(dir.join("syskit.log")).unwrap_or_default();
        let errlog = std::fs::read_to_string(dir.join("error.log")).unwrap_or_default();
        assert!(
            syskit.contains("smoke-probe-marker"),
            "info 未落 syskit.log"
        );
        assert!(syskit.contains("smoke-err-marker"), "error 未落 syskit.log");
        assert!(
            errlog.contains("smoke-err-marker"),
            "error 未分流 error.log"
        );
        assert!(
            !errlog.contains("smoke-probe-marker"),
            "info 不应进 error.log"
        );
    }
}
