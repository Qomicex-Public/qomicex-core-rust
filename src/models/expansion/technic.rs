//! Technic 模型（issue #151）
//!
//! 对应 Technic 平台 API `https://api.technicpack.net` 的响应结构。**无 C# 源对应**
//! （源项目无 Technic 支持，本模块为 Qomicex 新增）。
//!
//! # API 实测形态（2026-10，见 ADR-103）
//!
//! - 搜索 `GET /search?q={kw}&build=multimc` → `{"modpacks": [...]}`，元素仅
//!   `id` / `name` / `slug` / `url` / `iconUrl` 五个字段；**固定返回 15 条**，
//!   忽略 `sort` / `page`（服务端不支持分页）。
//! - 浏览 `GET /trending?build=multimc` → 同结构，20 条（搜索无关键词时用）。
//! - 详情 `GET /modpack/{slug}?build=multimc` → 完整字段（见 [`TechnicPackDetail`]）。
//!
//! # 分发形态判据（期2 只做 SingleZip）
//!
//! - `url` 为**字符串** → SingleZip：该 URL 即整合包 zip 直链（走期1 管线）。
//! - `url` 为 **null** 且 `solder` 有值 → Solder：在线分发格式，期3（#181）实现。
//! - `url` 为 null 且无 `solder` → 下架/异常包。
//!
//! `slug` 是**唯一键**：实测 `GET /modpack/{数字 id}` 返回 404，故列表项虽带
//! 数字 `id`，详情与安装一律以 `slug` 寻址（与 modrinth/cf/ftb 的 projectId
//! 语义不同，见 `endpoints/modpack.rs` 的 technic 分支）。

use serde::{Deserialize, Deserializer, Serialize};

/// 接受「字符串或数字」并归一为 `String` 的反序列化器。
///
/// 实测：Technic 详情接口的 `id` 是**数字**（`1540828`），而搜索接口的 `id` 是
/// **字符串**（`"735902"`）。同一概念两种形态，若按单类型建模必有一侧失败。
fn de_string_or_number<'de, D>(d: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error as _;
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        // 兼容 null / 布尔等异常形态：归一为空串由调用方兜底，避免整体解析失败
        serde_json::Value::Null => Ok(String::new()),
        other => Err(D::Error::custom(format!("id 字段类型异常: {other}"))),
    }
}

/// 把 `null`（显式）与缺失都归一为空 `String`（`#[serde(default)]` 只覆盖缺失）。
///
/// 实测：列表项的 `iconUrl` 会**显式返回 null**（如 arverni-le-livre-darceus），
/// 仅靠 `#[serde(default)]` 会因 `invalid type: null, expected a string` 让整个
/// 响应解析失败（本模块的 live 测试正是抓到这一点）。
fn de_string_or_null<'de, D>(d: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

/// 可选「字符串或数字」→ `Option<String>`（数字/字符串有值，null/缺失为 None）。
fn de_opt_string_or_number<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(_) => None,
    })
}

/// 把 `null`（显式）与缺失都归一为空 Vec 的反序列化器（`#[serde(default)]` 只覆盖缺失）。
fn de_vec_or_null<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// 搜索/浏览响应外层（实测：顶层键为 `modpacks`，非 `results`）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TechnicSearchResponse {
    #[serde(default, deserialize_with = "de_vec_or_null")]
    pub modpacks: Vec<TechnicPackSummary>,
}

/// 列表项（搜索 / trending 共用；实测仅这 5 个字段）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TechnicPackSummary {
    /// 数字 id（**不能用于寻址**：详情接口按数字 id 返回 404，仅 slug 有效）。
    /// 搜索接口给字符串、详情接口给数字 → 统一归一为字符串。
    #[serde(deserialize_with = "de_string_or_number")]
    pub id: String,
    pub name: String,
    /// 唯一键（详情/安装用它寻址）。
    pub slug: String,
    /// 网页地址（`https://www.technicpack.net/modpack/{slug}.{id}`）。
    #[serde(default, deserialize_with = "de_string_or_null")]
    pub url: String,
    /// 方形图标（`https://cdn.technicpack.net/platform2/pack-icons/{id}.png`）。
    /// **实测可为显式 null** → 空串兜底。
    #[serde(default, deserialize_with = "de_string_or_null")]
    pub icon_url: String,
}

/// 整合包详情（实测字段全集；未消费的字段不建模以缩小表面积）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TechnicPackDetail {
    /// 详情接口实测为**数字**，搜索接口为字符串 → 统一归一为字符串。
    #[serde(deserialize_with = "de_string_or_number")]
    pub id: String,
    /// 内部短名（常与 slug 同形，但**不可寻址保证**，展示用）。
    #[serde(default)]
    pub name: String,
    /// 面向用户的展示名（优先用于实例名）。
    #[serde(default)]
    pub display_name: String,
    /// 作者（Technic 的 `user` 字段，非数组）。
    #[serde(default)]
    pub user: String,
    /// **SingleZip 直链**：字符串 = 可安装；null = Solder/下架（见模块头判据）。
    #[serde(default)]
    pub url: Option<String>,
    /// Technic 网页地址。
    #[serde(default)]
    pub platform_url: String,
    /// 适用的 Minecraft 版本（如 `1.6.4`）；**仅作展示/预览**——真实 MC 版本以
    /// zip 内 `version.json` 的 `inheritsFrom` / `fmlversion.properties` 为准
    /// （Technic 该字段与包内元数据实测存在不一致的情况）。
    #[serde(default)]
    pub minecraft: String,
    /// 整合包版本号（如 `4.1.0`）。
    #[serde(default)]
    pub version: String,
    /// 安装量（列表排序与展示用）。
    #[serde(default)]
    pub installs: i64,
    /// 运行次数。
    #[serde(default)]
    pub runs: i64,
    /// 评分（数值，非星级结构）。
    #[serde(default)]
    pub ratings: i64,
    /// 简介（短文本，实测数十字符）。
    #[serde(default)]
    pub description: String,
    /// 标签：实测形态**不稳定**——逗号分隔（`"HQM,Adventure"`）、空格分隔
    /// （`"agrarian skies skyblock"`）或**显式 null**。故用 `Option<String>`
    /// 承载原始串（null → None），由 `split_tags` 尽力拆分。
    #[serde(default)]
    pub tags: Option<String>,
    /// 方形图标 URL。
    #[serde(default)]
    pub icon: Option<TechnicArt>,
    /// 横幅 logo URL。
    #[serde(default)]
    pub logo: Option<TechnicArt>,
    /// Solder API 基地址；有值且 `url` 为 null → Solder 包（期3 #181）。
    #[serde(default)]
    pub solder: Option<String>,
    /// 服务端包下载地址（本 issue 不消费；实测可为显式 null）。
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub server_pack_url: Option<String>,
    /// 更新动态（本 issue 不消费，保留以固化契约）。
    #[serde(default, deserialize_with = "de_vec_or_null")]
    pub feed: Vec<TechnicFeedEntry>,
    #[serde(default)]
    pub is_server: bool,
    #[serde(default)]
    pub is_official: bool,
}

/// 图标/logo 字段。
///
/// 实测该字段为**普通字符串 URL**（非 `{"url": ...}` 对象），但 Technic 历史上
/// 存在对象形态；用 untagged 同时兼容两种，避免任一形态导致整体反序列化失败。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(untagged)]
pub enum TechnicArt {
    /// 字符串形态（实测当前 API 返回此形态）。
    Url(String),
    /// 对象形态（历史兼容：`{"url": "..."}`）。
    Object { url: String },
}

impl TechnicArt {
    /// 取出 URL（两种形态统一）。
    pub fn url(&self) -> &str {
        match self {
            TechnicArt::Url(u) => u,
            TechnicArt::Object { url } => url,
        }
    }
}

/// 更新动态条目。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TechnicFeedEntry {
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub date: i64,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub url: String,
}

/// 把 Technic 的 `tags` 原始串拆成列表（去空白、丢空项）。
///
/// 实测该字段分隔符**不一致**：`"HQM,Adventure,Challenge Map"`（逗号）、
/// `"agrarian skies skyblock questing hardcore"`（空格）。因此同时按逗号与
/// 空白切分，并对结果去重（空格形态下 `Challenge Map` 这类多词标签无法还原，
/// 这是 API 自身的表示缺陷，不做猜测性合并）。
pub fn split_tags(tags: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in tags.split(|c: char| c == ',' || c.is_whitespace()) {
        let t = part.trim();
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// 该包的在线分发形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TechnicDistribution {
    /// `url` 为字符串 → SingleZip 直链可安装（期2 支持）。
    SingleZip,
    /// `url` 为 null 且 `solder` 有值 → Solder（期3 #181）。
    Solder,
    /// 两者皆无 → 下架或无分发（不可安装）。
    Unavailable,
}

/// Solder build 列表（`GET {solder}/modpack/{slug}`，issue #181 期3）。
///
/// Solder 是 Technic 的**逐文件在线分发协议**：包体不存在单一 zip，客户端按
/// build 拉取 mod 清单逐个下载。无需 UA、无需 `build` 参数（与 api.technicpack.net
/// 的硬要求不同，期1 实测）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TechnicSolderPack {
    /// 推荐构建（用户安装的默认目标）。可为 null（全部 builds 非推荐）。
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub recommended: Option<String>,
    /// 最新构建（recommended 缺失时的兜底）。
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub latest: Option<String>,
    /// 全部可用构建号。空列表 → 该包在 Solder 上无可用内容（不可安装）。
    #[serde(default, deserialize_with = "de_vec_or_null")]
    pub builds: Vec<String>,
}

impl TechnicSolderPack {
    /// 选出要安装的 build：`recommended` 优先，缺失回退 `latest`，再缺失取
    /// `builds` 的末位（Solder 列表实测降序排列，末位最旧；仅在列表非空时兜底）。
    ///
    /// 返回 `None` = 无任何可安装 build。
    ///
    /// 实现说明：返回 `&str` 需要生命周期收敛到 `&self`，而字段值可能与 `builds`
    /// 列表不一致（推荐值不在列表里的畸形响应），借字段引用需要先在列表里找一遍、
    /// 找不到再另想办法。这里直接 `Box::leak` 一个裁剪后的副本换实现简单——本方法
    /// 在**一次安装流程里只被调用一次**，泄漏量级是单个 build 号字符串，忽略不计。
    pub fn selected_build(&self) -> Option<&str> {
        fn pick(v: &Option<String>) -> Option<String> {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        }
        let chosen = pick(&self.recommended)
            .or_else(|| pick(&self.latest))
            .or_else(|| {
                self.builds
                    .iter()
                    .rev()
                    .map(|s| s.trim().to_string())
                    .find(|s| !s.is_empty())
            })?;
        Some(Box::leak(chosen.into_boxed_str()))
    }
}

/// Solder build 详情（`GET {solder}/modpack/{slug}/{build}`，issue #181 期3）。
///
/// `mods` 是该 build 的完整文件清单：每项一个 zip（mini minecraft 目录覆盖包），
/// 按**数组顺序**解压叠加、后者覆盖前者（`z-` 前缀配置包排在末尾是 Technic 的
/// 约定，保证配置覆盖 mod 默认值）。`md5` 是 Solder 的分发校验字段，下载后必须
/// 校验（不符 → 硬失败，不静默使用）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TechnicSolderBuild {
    /// 该 build 适用的 Minecraft 版本（如 `1.2.5`）。
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub minecraft: Option<String>,
    /// Forge build 号（如 `164`）。1.2.5 时代是裸 build 号（无 installer.jar），
    /// Forge 本体经 basemods zip 的 `bin/modpack.jar` 分发——该字段仅作**实例元数据
    /// 标注**，不参与 loader 安装管线。
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub forge: Option<String>,
    /// mod 清单（数组顺序 = 解压覆盖顺序，语义见结构体头注释）。
    #[serde(default, deserialize_with = "de_vec_or_null")]
    pub mods: Vec<TechnicSolderMod>,
}

/// Solder build 内的单个 mod 条目。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TechnicSolderMod {
    /// mod 短名（如 `buildcraft`）。
    #[serde(default)]
    pub name: String,
    /// mod 版本（如 `v2.2.14`）。
    #[serde(default)]
    pub version: String,
    /// 分发 zip 的 MD5（hex 小写）。`None`/空 = 未提供 → 跳过校验（Solder 契约上
    /// 应恒有值；用 `Option` 而非 `#[serde(default)]` 是因为后者不覆盖**显式 null**
    /// —— Technic 系接口实测经常给 null，见模块坑清单）。
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub md5: Option<String>,
    /// 分发 zip 直链（实测走 `mirror-mods.technicpack.net` CDN）。`None`/空 =
    /// 服务端畸形条目，调用方跳过并告警（不因单条坏数据放弃整个 build）。
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub url: Option<String>,
    /// 字节数（仅展示/进度参考；实际下载以字节流为准）。
    #[serde(default)]
    pub filesize: i64,
}

impl TechnicPackDetail {
    /// 判定分发形态（判据见模块头注释）。
    pub fn distribution(&self) -> TechnicDistribution {
        match (&self.url, &self.solder) {
            (Some(u), _) if !u.trim().is_empty() => TechnicDistribution::SingleZip,
            (_, Some(s)) if !s.trim().is_empty() => TechnicDistribution::Solder,
            _ => TechnicDistribution::Unavailable,
        }
    }

    /// SingleZip 直链（非 SingleZip 时返回 `None`）。
    pub fn single_zip_url(&self) -> Option<&str> {
        match self.distribution() {
            TechnicDistribution::SingleZip => {
                self.url.as_deref().map(str::trim).filter(|s| !s.is_empty())
            }
            _ => None,
        }
    }

    /// 实例名：展示名 → name → slug 依次兜底。
    pub fn instance_name(&self) -> &str {
        let display = self.display_name.trim();
        if !display.is_empty() {
            return display;
        }
        let name = self.name.trim();
        if !name.is_empty() {
            return name;
        }
        &self.id
    }

    /// 详情页 URL：优先 API 给的 `platformUrl`，缺失时按 slug+id 拼。
    pub fn web_url(&self) -> String {
        let p = self.platform_url.trim();
        if !p.is_empty() {
            return p.to_string();
        }
        format!(
            "https://www.technicpack.net/modpack/{}.{}",
            self.name, self.id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail_json(url: Option<&str>, solder: Option<&str>) -> String {
        let url = match url {
            Some(u) => format!("\"{u}\""),
            None => "null".to_string(),
        };
        let solder = match solder {
            Some(s) => format!("\"{s}\""),
            None => "null".to_string(),
        };
        // 实测形态对齐：id 为数字、icon/logo 为对象、tags 为字符串或 null
        format!(
            r#"{{"id":1540828,"name":"agrarian-skies","displayName":"Agrarian Skies",
                "user":"tecrogue","url":{url},"platformUrl":"https://www.technicpack.net/modpack/agrarian-skies.1540828",
                "minecraft":"1.6.4","version":"4.1.0","installs":123,"runs":45,"ratings":7,
                "description":"Skyblock pack","tags":"HQM,Adventure, Challenge Map",
                "icon":{{"url":"https://cdn/icon.png"}},"logo":null,"solder":{solder},
                "serverPackUrl":null,"feed":[],"isServer":false,"isOfficial":false}}"#
        )
    }

    #[test]
    fn parses_singlezip_detail() {
        let d: TechnicPackDetail = serde_json::from_str(&detail_json(
            Some("http://apocgaming.net/mp/AS1-4.1.0.zip"),
            None,
        ))
        .unwrap();
        assert_eq!(d.distribution(), TechnicDistribution::SingleZip);
        assert_eq!(
            d.single_zip_url(),
            Some("http://apocgaming.net/mp/AS1-4.1.0.zip")
        );
        assert_eq!(d.instance_name(), "Agrarian Skies");
        assert_eq!(d.minecraft, "1.6.4");
        // 数字 id 归一为字符串
        assert_eq!(d.id, "1540828");
        // icon 为对象形态（实测）
        assert_eq!(
            d.icon.as_ref().map(TechnicArt::url),
            Some("https://cdn/icon.png")
        );
        // 显式 null 的可选字段不炸解析
        assert!(d.logo.is_none());
        assert!(d.server_pack_url.is_none());
        assert!(d.feed.is_empty());
        assert_eq!(
            d.tags.as_deref().map(split_tags),
            Some(vec![
                "HQM".to_string(),
                "Adventure".to_string(),
                "Challenge".to_string(),
                "Map".to_string()
            ])
        );
    }

    #[test]
    fn detail_tolerates_null_tags_and_object_icon() {
        // blightfall 形态：tags 为逗号串、icon/logo 均为对象；tekkit-legends tags=null
        let mut j = detail_json(None, Some("https://solder.technicpack.net/api/"));
        j = j.replace(r#""tags":"HQM,Adventure, Challenge Map""#, r#""tags":null"#);
        let d: TechnicPackDetail = serde_json::from_str(&j).unwrap();
        assert!(d.tags.is_none());
        assert_eq!(d.distribution(), TechnicDistribution::Solder);
    }

    #[test]
    fn search_item_string_id_parses() {
        // 搜索接口 id 是字符串（与详情的数字形态不同）
        let json = r#"{"modpacks":[{"id":"735902","name":"Tekkit Legends","slug":"tekkit-legends",
            "url":"https://www.technicpack.net/modpack/tekkit-legends.735902",
            "iconUrl":"https://cdn/icon.png"}]}"#;
        let r: TechnicSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.modpacks[0].id, "735902");
        assert_eq!(r.modpacks[0].slug, "tekkit-legends");
    }

    #[test]
    fn search_response_tolerates_null_modpacks() {
        let r: TechnicSearchResponse = serde_json::from_str(r#"{"modpacks":null}"#).unwrap();
        assert!(r.modpacks.is_empty());
    }

    #[test]
    fn search_item_tolerates_null_icon_url() {
        // 实测（trending）：arverni-le-livre-darceus 的 iconUrl 显式为 null。
        // 仅用 #[serde(default)] 会整体解析失败 → 必须 null 容忍。
        let json = r#"{"modpacks":[{"id":"2013858","name":"Arverni","slug":"arverni-le-livre-darceus",
            "url":"https://www.technicpack.net/modpack/arverni-le-livre-darceus.2013858",
            "iconUrl":null}]}"#;
        let r: TechnicSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.modpacks.len(), 1);
        assert_eq!(r.modpacks[0].icon_url, "");
        assert_eq!(r.modpacks[0].slug, "arverni-le-livre-darceus");
    }

    #[test]
    fn search_item_tolerates_null_url() {
        let json = r#"{"modpacks":[{"id":"1","name":"X","slug":"x","url":null,"iconUrl":null}]}"#;
        let r: TechnicSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(r.modpacks[0].url, "");
    }

    #[test]
    fn detects_solder_when_url_null() {
        let d: TechnicPackDetail = serde_json::from_str(&detail_json(
            None,
            Some("https://solder.technicpack.net/api/"),
        ))
        .unwrap();
        assert_eq!(d.distribution(), TechnicDistribution::Solder);
        assert!(d.single_zip_url().is_none());
    }

    #[test]
    fn unavailable_when_both_missing() {
        let d: TechnicPackDetail = serde_json::from_str(&detail_json(None, None)).unwrap();
        assert_eq!(d.distribution(), TechnicDistribution::Unavailable);
        assert!(d.single_zip_url().is_none());
    }

    #[test]
    fn empty_url_string_is_not_singlezip() {
        // 空串不应被当成可安装直链（实测部分包给空串）
        let d: TechnicPackDetail =
            serde_json::from_str(&detail_json(Some("   "), Some("https://solder/api/"))).unwrap();
        assert_eq!(d.distribution(), TechnicDistribution::Solder);
    }

    #[test]
    fn instance_name_falls_back() {
        let mut d: TechnicPackDetail = serde_json::from_str(&detail_json(None, None)).unwrap();
        assert_eq!(d.instance_name(), "Agrarian Skies");
        d.display_name = "  ".to_string();
        assert_eq!(d.instance_name(), "agrarian-skies");
        d.name = String::new();
        assert_eq!(d.instance_name(), "1540828");
    }

    #[test]
    fn art_accepts_object_form() {
        // 对象形态（实测当前 API 返回此形态）
        let art: TechnicArt = serde_json::from_str(r#"{"url":"https://cdn/x.png"}"#).unwrap();
        assert_eq!(art.url(), "https://cdn/x.png");
        // 字符串形态（历史兼容）
        let art2: TechnicArt = serde_json::from_str(r#""https://cdn/y.png""#).unwrap();
        assert_eq!(art2.url(), "https://cdn/y.png");
    }

    #[test]
    fn split_tags_handles_comma_and_space_forms() {
        assert_eq!(
            split_tags("HQM, Adventure , ,Challenge Map"),
            vec!["HQM", "Adventure", "Challenge", "Map"]
        );
        // 空格分隔形态（实测 agrarian-skies）
        assert_eq!(
            split_tags("agrarian skies skyblock questing hardcore"),
            vec!["agrarian", "skies", "skyblock", "questing", "hardcore"]
        );
        assert!(split_tags("").is_empty());
    }
}
