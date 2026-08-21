//! 本地资源工厂 + 本地资源基类共享逻辑（B10）
//!
//! 对应源文件：
//! - Services/Expansion/Local/DefaultLocalModsFactory.cs
//!   （`DefaultLocalResourcesFactory : ILocalResourcesFactory`，工厂按参数分派 6 类管理器）
//! - Services/Expansion/Local/LocalResourceBase.cs
//!   （`LocalResourceBase` 基类：哈希指纹 + ZIP 内文件读取；哈希已移植 util/murmurhash2.rs，
//!   此处委托；目录解析（版本分段目录）实际位于各管理器类内（如 Mods.cs 的
//!   `ModDirectory => _versionSegmented ? gameDir/versions/{version}/mods : gameDir/mods`），
//!   由各管理器任务实现，本模块不重复）

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::api::local::{
    DataPacksManager, LocalResourcesFactory, ModsManager, ResourcepackManager, SavesManager,
    ScreenshotsManager, ShadersManager,
};
use crate::util::murmurhash2::{curse_forge_fingerprint, murmur_hash2};

use super::datapacks::DataPacks;
use super::mods::Mods;
use super::resourcepacks::ResourcepackService;
use super::saves::Saves;
use super::screenshots::Screenshots;
use super::shaders::ShadersService;

/// 本地资源工厂（源：`DefaultLocalResourcesFactory`，DefaultLocalModsFactory.cs）。
/// 持有共享的 HTTP 客户端，按调用参数创建各类本地内容管理器实例。
///
/// 分派语义：C# 每次 create 调用 `new Xxx(...)` 创建新实例并共享同一个 HttpClient
/// （引用传递）→ Rust 返回 `Box<dyn XxxManager + Send + Sync>` 转移所有权；
/// `reqwest::Client` 内部为 Arc 包装的轻量克隆，`clone()` 保持"共享同一客户端"语义
/// （同 api/installer.rs 的 CreateFtbModpack 映射决策）。
///
/// ⚠️ 签名差异（翻译日志 p18 后修订）：源方法首参为 `gameDir`（按实例目录），早期
/// Rust 翻译由工厂持有的 `game_root` 提供 → 实例拥有独立 .minecraft 目录时扫错目录
/// （主仓 instance_files.rs resolve() 按实例解析，全局 game_root 对不上）→ 已移除
/// `game_root` 字段，game_dir 由调用方每次显式传入。
pub(crate) struct DefaultLocalResourcesFactory {
    /// 共享 HTTP 客户端（源：`_http`，HttpClient）
    http: reqwest::Client,
    /// 图标缓存目录（源无对应；Rust 新增：per-jar 图标内容哈希磁盘缓存）
    icon_cache_dir: Option<PathBuf>,
}

impl DefaultLocalResourcesFactory {
    /// 创建工厂（源：`DefaultLocalResourcesFactory(HttpClient http)`）
    pub(crate) fn new(http: reqwest::Client, icon_cache_dir: Option<PathBuf>) -> Self {
        Self {
            http,
            icon_cache_dir,
        }
    }
}

impl LocalResourcesFactory for DefaultLocalResourcesFactory {
    /// 创建 Mod 管理器（源：`CreateMods(string gameDir, string version, bool versionSegmented, string apiKey)`）
    fn create_mods(
        &self,
        game_dir: &str,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn ModsManager + Send + Sync> {
        Box::new(Mods::new(
            self.http.clone(),
            game_dir.to_string(),
            version.to_string(),
            version_segmented,
            api_key.to_string(),
            self.icon_cache_dir.clone(),
        ))
    }

    /// 创建存档管理器（源：`CreateSaves(string gameDir, string version, bool versionSegmented, string apiKey)`）
    fn create_saves(
        &self,
        game_dir: &str,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn SavesManager + Send + Sync> {
        Box::new(Saves::new(
            self.http.clone(),
            game_dir.to_string(),
            version.to_string(),
            version_segmented,
            api_key.to_string(),
        ))
    }

    /// 创建资源包管理器（源：`CreateResourcepack(string gameDir, string version, bool versionSegmented, string apiKey)`）
    fn create_resourcepack(
        &self,
        game_dir: &str,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn ResourcepackManager + Send + Sync> {
        Box::new(ResourcepackService::new(
            self.http.clone(),
            game_dir.to_string(),
            version.to_string(),
            version_segmented,
            api_key.to_string(),
        ))
    }

    /// 创建光影管理器（源：`CreateShaders(string gameDir, string version, bool versionSegmented, string apiKey)`）
    fn create_shaders(
        &self,
        game_dir: &str,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn ShadersManager + Send + Sync> {
        Box::new(ShadersService::new(
            self.http.clone(),
            game_dir.to_string(),
            version.to_string(),
            version_segmented,
            api_key.to_string(),
        ))
    }

    /// 创建截图管理器（源：`CreateScreenshots(string gameDir, string version, bool versionSegmented, string apiKey)`）
    fn create_screenshots(
        &self,
        game_dir: &str,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn ScreenshotsManager + Send + Sync> {
        Box::new(Screenshots::new(
            self.http.clone(),
            game_dir.to_string(),
            version.to_string(),
            version_segmented,
            api_key.to_string(),
        ))
    }

    /// 创建数据包管理器（源：`CreateDataPacks(string gameDir, string version, bool versionSegmented, string apiKey)`）
    fn create_data_packs(
        &self,
        game_dir: &str,
        version: &str,
        version_segmented: bool,
        api_key: &str,
    ) -> Box<dyn DataPacksManager + Send + Sync> {
        Box::new(DataPacks::new(
            self.http.clone(),
            game_dir.to_string(),
            version.to_string(),
            version_segmented,
            api_key.to_string(),
        ))
    }

    /// 创建服务器管理器（源：ContentService.CreateServerManager(gameDir, version, versionSpecific)）
    fn create_server_manager(
        &self,
        game_dir: &str,
        version: &str,
        version_specific: bool,
    ) -> Box<dyn crate::api::server::ServerManager + Send + Sync> {
        Box::new(crate::services::server::servers_dat::ServerManager::new(
            game_dir.to_string(),
            version.to_string(),
            version_specific,
        ))
    }
}

/// 本地资源基类共享逻辑（源：`LocalResourceBase`，LocalResourceBase.cs）。
/// 源为静态成员基类，由各管理器（Mods/Saves/... : LocalResourceBase）继承调用 →
/// Rust 以单元结构体命名空间 + 关联函数承载（各管理器 `use super::factory::LocalResourceBase;`
/// 后以 `LocalResourceBase::xxx` 调用，保持源调用形态）。
///
/// ⚠️ 与源差异说明：源基类**不含**目录解析——版本分段目录
/// （`gameDir/mods` 或 `gameDir/versions/{version}/mods`）是各管理器类自身的
/// 属性（如 Mods.cs 的 `ModDirectory`），由对应并行任务实现。
#[allow(dead_code)] // 待 B10 各管理器（并行任务）接入后解除
pub(crate) struct LocalResourceBase;

impl LocalResourceBase {
    /// CurseForge 文件指纹（源：`CurseForgeFingerprint(byte[])`）。
    /// 已移植至 util/murmurhash2.rs，此处委托保持基类调用形态
    #[allow(dead_code)] // 待各管理器接入
    pub(crate) fn curse_forge_fingerprint(data: &[u8]) -> i64 {
        curse_forge_fingerprint(data)
    }

    /// MurmurHash2（源：`MurmurHash2(byte[], uint seed = 1)`）。
    /// 已移植至 util/murmurhash2.rs（seed 默认值由调用方显式传入），此处委托
    #[allow(dead_code)] // 待各管理器接入
    pub(crate) fn murmur_hash2(data: &[u8], seed: u32) -> i64 {
        murmur_hash2(data, seed)
    }

    /// 从 ZIP 压缩包内读取指定文件内容（源：`TryReadFileFromZip(string path, string fileName)`）。
    /// 文件名不区分大小写匹配（源 `StringComparison.OrdinalIgnoreCase` → ASCII 忽略大小写）；
    /// 文件不存在 / 不是有效 ZIP / 无匹配条目 / 读取失败均返回 `None`（源 catch-all → null）。
    /// 返回 `Option<Vec<u8>>`（源 `byte[]?`）
    #[allow(dead_code)] // 待各管理器接入
    pub(crate) fn try_read_file_from_zip(path: &Path, file_name: &str) -> Option<Vec<u8>> {
        let file = File::open(path).ok()?;
        let mut archive = ZipArchive::new(file).ok()?;
        // 源按 FullName 忽略大小写查找首个匹配条目 → 先定位索引再读取
        let index = archive
            .file_names()
            .position(|name| name.eq_ignore_ascii_case(file_name))?;
        let mut entry = archive.by_index(index).ok()?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).ok()?;
        Some(buf)
    }
}
