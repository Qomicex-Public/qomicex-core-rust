//! 本地内容管理模型（B1）
//! 对应源：Models/Expansion/Local/ 下 6 个文件
//! (DataPackInfo.cs, ModInfo.cs, ResourcePackInfo.cs, SaveInfo.cs, ScreenshotInfo.cs, ShaderInfo.cs)

use serde::{Deserialize, Serialize};
use std::path::Path;

/// 表示一个本地数据包（Data Pack）的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DataPackInfo {
    /// 数据包名称
    pub name: String,
    /// 数据包描述
    pub description: String,
    /// 数据包版本
    pub version: String,
    /// 数据包文件路径
    pub file_path: String,
    /// 是否为目录形式
    pub is_directory: bool,
    /// 数据包格式版本
    pub pack_format: i32,
    /// 图标路径
    pub icon: String,
    /// CurseForge 项目 ID
    pub curse_forge_id: i32,
    /// Modrinth 项目 ID
    pub modrinth_id: String,
    /// SHA1 哈希
    pub sha1_hash: String,
    /// CurseForge 指纹哈希
    pub cf_hash: i64,
}

/// 表示一个本地 Mod 的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModInfo {
    /// Mod 名称
    pub name: String,
    /// Mod 描述
    pub description: String,
    /// Mod 版本
    pub version: String,
    /// 作者列表
    pub authors: Vec<String>,
    /// Mod 文件路径
    pub file_path: String,
    /// 图标路径
    pub icon: String,
    /// CurseForge 项目 ID
    pub curse_forge_id: i32,
    /// Modrinth 项目 ID
    pub modrinth_id: String,
    /// SHA1 哈希
    pub sha1_hash: String,
    /// CurseForge 指纹哈希
    pub cf_hash: i64,
}

impl ModInfo {
    /// 是否为激活状态（源为 get-only 计算属性 `Active`，判断扩展名是否为 .jar）
    /// ⚠️ UNMAPPED：计算属性无法映射为 serde 字段，此处保留逻辑为方法，不参与序列化
    pub fn is_active(&self) -> bool {
        Path::new(&self.file_path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
    }
}

/// 表示一个本地资源包（Resource Pack）的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourcePackInfo {
    /// 资源包名称
    pub name: String,
    /// 资源包描述
    pub description: String,
    /// 资源包版本
    pub version: String,
    /// 资源包文件路径
    pub file_path: String,
    /// 是否为目录形式
    pub is_directory: bool,
    /// 资源包格式版本
    pub pack_format: i32,
    /// 图标路径
    pub icon: String,
    /// CurseForge 项目 ID
    pub curse_forge_id: i32,
    /// Modrinth 项目 ID
    pub modrinth_id: String,
    /// SHA1 哈希
    pub sha1_hash: String,
    /// CurseForge 指纹哈希
    pub cf_hash: i64,
}

/// 表示一个本地存档的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SaveInfo {
    /// 存档名称
    pub name: String,
    /// 存档文件路径
    pub file_path: String,
    /// 最后游玩时间戳
    pub last_played: i64,
    /// 图标路径
    pub icon: String,
}

/// 表示一个本地截图的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotInfo {
    /// 截图文件路径
    pub file_path: String,
    /// 截图文件名
    pub file_name: String,
    /// 创建时间（源为 DateTime，暂用原始字符串保真，类型决策见日志 ⚠️ UNMAPPED）
    pub created_at: String,
    /// 文件大小（字节）
    pub file_size: i64,
}

/// 表示一个本地光影包（Shader）的信息
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShaderInfo {
    /// 光影包名称
    pub name: String,
    /// 光影包描述
    pub description: String,
    /// 光影包版本
    pub version: String,
    /// 光影包文件路径
    pub file_path: String,
    /// 图标路径
    pub icon: String,
    /// CurseForge 项目 ID
    pub curse_forge_id: i32,
    /// Modrinth 项目 ID
    pub modrinth_id: String,
    /// SHA1 哈希
    pub sha1_hash: String,
    /// CurseForge 指纹哈希
    pub cf_hash: i64,
}
