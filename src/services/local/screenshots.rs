//! 截图扫描（B10，对应源：Services/Expansion/Local/Screenshots.cs）
//!
//! 源类 `Screenshots : LocalResourceBase`（构造函数注入
//! `HttpClient http, string gameDirectory, string version, bool versionSegmented, string apiKey`）。
//! 实现 `ScreenshotsManager::get_screenshot_list`（api/local.rs）：
//! 1. 扫描截图目录下的 *.png 文件，版本分段目录为
//!    `{gameDirectory}/versions/{version}/screenshots`，否则 `{gameDirectory}/screenshots`
//! 2. 每条目组装 ScreenshotInfo：路径 / 文件名 / 创建时间 / 文件大小
//!
//! ⚠️ UNMAPPED（U4）：`CreatedAt` 源为 `FileInfo.CreationTime`（本地时区 DateTime，
//! 默认 ToString 格式随文化/时区变化）→ 模型定案 `created_at: String` 原始字符串保真
//! （见 models/expansion/local.rs）；无 chrono 依赖（B2 定案）→ 输出 UTC 时刻的
//! 固定格式 `YYYY-MM-DDTHH:MM:SS`（无时区后缀），与源本地时间字符串存在时区/精度差异，
//! 见日志 D5。
//!
//! Android 兼容性：纯 Rust（std + 既有依赖），无新增 C 依赖。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::api::local::ScreenshotsManager;
use crate::models::expansion::local::ScreenshotInfo;

/// 截图管理器实现（源：class `Screenshots : LocalResourceBase`，Screenshots.cs）。
/// 命名对齐 P44 factory.rs 引用点（`super::screenshots::Screenshots`）。
pub(crate) struct Screenshots {
    /// HTTP 客户端（源：`_http` HttpClient）。
    /// ⚠️ 源类持有但方法体未使用（截图列表无网络调用），保留字段以对齐构造签名
    #[allow(dead_code)]
    http: reqwest::Client,
    /// 游戏根目录（源：`_gameDirectory` gameDirectory）
    game_root: String,
    /// 游戏版本（源：`_version` version）
    version: String,
    /// 版本分段目录（源：`_versionSegmented` versionSegmented）
    version_segmented: bool,
    /// CurseForge API Key（源：`_apiKey` apiKey）。
    /// ⚠️ 同 http：源类持有但方法体未使用，保留字段以对齐构造签名
    #[allow(dead_code)]
    api_key: String,
}

impl Screenshots {
    /// 创建截图管理器（源：构造函数 `Screenshots(HttpClient, string, string, bool, string)`）
    pub(crate) fn new(
        http: reqwest::Client,
        game_root: String,
        version: String,
        version_segmented: bool,
        api_key: String,
    ) -> Self {
        Self {
            http,
            game_root,
            version,
            version_segmented,
            api_key,
        }
    }

    /// 截图目录（源：`_versionSegmented
    ///   ? Path.Combine(_gameDirectory, "versions", _version, "screenshots")
    ///   : Path.Combine(_gameDirectory, "screenshots")`）
    fn screenshot_directory(&self) -> PathBuf {
        if self.version_segmented {
            Path::new(&self.game_root)
                .join("versions")
                .join(&self.version)
                .join("screenshots")
        } else {
            Path::new(&self.game_root).join("screenshots")
        }
    }

    /// 扫描截图文件（源：GetScreenshotFiles，仅 *.png 文件）。
    /// 目录不存在 → 空列表（源 `return [];`）。
    /// ⚠️ 差异：源 `Directory.GetFiles` 异常直接传播（GetScreenshotList 无 try/catch），
    /// 但 trait 签名 `fn get_screenshot_list(&self) -> Vec<ScreenshotInfo>`（api/local.rs
    /// 定案）无 Result 可表达 → 枚举失败记日志并返回空列表（D6，收敛语义）。
    fn get_screenshot_files(&self) -> Vec<PathBuf> {
        let screenshot_directory = self.screenshot_directory();
        if !screenshot_directory.is_dir() {
            return Vec::new();
        }

        let mut files: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(&screenshot_directory).into_iter().flatten() {
            match entry {
                Ok(entry) => {
                    let path = entry.path();
                    // 源模式 "*.png"：文件名以 .png 结尾（Windows 上不区分大小写 → 忽略大小写匹配）
                    if path.is_file()
                        && path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
                    {
                        files.push(path);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "读取截图目录失败（{}）：{e}",
                        screenshot_directory.display()
                    );
                    break;
                }
            }
        }
        files
    }
}

impl ScreenshotsManager for Screenshots {
    /// 扫描截图列表（源：GetScreenshotList）。
    /// 元数据读取失败（源 FileInfo.Length/CreationTime 访问异常）→ 跳过该条目
    /// （trait 无 Result，D6 收敛：日志 + 部分结果）。
    fn get_screenshot_list(&self) -> Vec<ScreenshotInfo> {
        let files = self.get_screenshot_files();
        let mut screenshot_infos: Vec<ScreenshotInfo> = Vec::new();

        for file in &files {
            let metadata = match std::fs::metadata(file) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("读取截图元数据失败（{}）：{e}", file.display());
                    continue;
                }
            };

            screenshot_infos.push(ScreenshotInfo {
                file_path: file.to_string_lossy().into_owned(),
                file_name: file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                created_at: created_at_string(&metadata.created(), &metadata.modified()),
                file_size: metadata.len() as i64,
            });
        }

        screenshot_infos
    }
}

/// 创建时间字符串（源：`fileInfo.CreationTime` → DateTime → ToString()）。
/// ⚠️ UNMAPPED（U4）：模型 `created_at: String` 原始字符串保真；std 无本地时区 API →
/// 取 UTC 时刻固定格式 `YYYY-MM-DDTHH:MM:SS`（无时区后缀）。
/// `created()` 在部分平台/文件系统不可用（std 返回 Err）→ 回退 `modified()`，
/// 两者皆失败 → 空字符串（D5）。
fn created_at_string(
    created: &Result<SystemTime, std::io::Error>,
    modified: &Result<SystemTime, std::io::Error>,
) -> String {
    let time = match created {
        Ok(t) => *t,
        Err(_) => match modified {
            Ok(t) => *t,
            Err(_) => return String::new(),
        },
    };
    let Ok(duration) = time.duration_since(std::time::UNIX_EPOCH) else {
        return String::new();
    };
    format_unix_seconds(duration.as_secs() as i64)
}

/// Unix 秒 → `YYYY-MM-DDTHH:MM:SS`（UTC，无时区后缀；民用日期换算采用
/// Howard Hinnant civil_from_days 算法，同 launch/process.rs unix_to_utc_string）。
fn format_unix_seconds(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, minute, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{sec:02}"
    )
}

