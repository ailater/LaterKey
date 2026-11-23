//! 单实例锁：两个 laterkey 同时跑会互相改写事件，必须避免。
//!
//! 用文件锁（flock）：进程消失（崩溃、被 kill）时锁自动释放，不会把后来
//! 的实例锁死在一份陈旧状态上——这对 KeepAlive 的 LaunchAgent 是必需的。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

#[derive(Debug)]
pub struct InstanceLock {
    file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// 抢锁（生产路径）。已有实例持有时返回 Err（里面写着对方 pid）。
    pub fn acquire() -> Result<Self, String> {
        Self::acquire_at(lock_path())
    }

    /// 在指定路径抢锁。测试用独立路径：生产常驻进程持有默认锁时，
    /// 用同一路径的测试会失败（也会干扰运行中的实例）。
    pub fn acquire_at(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| format!("打不开实例锁 {}：{e}", path.display()))?;

        // SAFETY: flock 的常规用法；fd 由 file 持有
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let other = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .filter(|&pid| process_alive(pid));
            return Err(match other {
                Some(pid) => format!("已有 laterkey 实例在运行（pid {pid}），不能重复启动"),
                None => "已有 laterkey 实例在运行，不能重复启动".into(),
            });
        }

        let _ = file.set_len(0);
        let _ = writeln!(file, "{}", std::process::id());
        Ok(Self { file, path })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // SAFETY: 自己持有的 fd
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Library/Application Support/laterkey/laterkey.pid")
}

fn process_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) 只做存在性探测
    unsafe {
        libc::kill(pid as i32, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_is_rejected_and_first_exit_releases() {
        let path =
            std::env::temp_dir().join(format!("laterkey-test-lock-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let a = InstanceLock::acquire_at(path.clone()).unwrap();
        let err = InstanceLock::acquire_at(path.clone()).unwrap_err();
        assert!(err.contains("不能重复启动"), "{err}");
        drop(a);
        // 第一个退出后锁释放，可以再来一个
        let b = InstanceLock::acquire_at(path.clone()).unwrap();
        drop(b);
        let _ = std::fs::remove_file(&path);
    }
}
