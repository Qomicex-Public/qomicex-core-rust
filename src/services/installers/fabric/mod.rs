// Copyright (C) 2026 Qomicex Launcher Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fabric 安装器
pub mod install;

/// Fabric meta 官方源根地址。
const META_OFFICIAL: &str = "https://meta.fabricmc.net/";
/// Fabric meta 的 BMCLAPI 镜像根地址（路径与官方同构）。
const META_BMCLAPI: &str = "https://bmclapi2.bangbang93.com/fabric-meta/";
/// Fabric maven 官方源根地址（loader 库）。
const MAVEN_OFFICIAL: &str = "https://maven.fabricmc.net/";
/// Fabric maven 的 BMCLAPI 镜像根地址（`/maven` 前缀，其余路径同构）。
const MAVEN_BMCLAPI: &str = "https://bmclapi2.bangbang93.com/maven/";

/// Fabric meta/maven 地址的「另一侧」地址（官方 ↔ BMCLAPI），未识别返回 `None`。
///
/// 与 [`meta_candidates`] 同样的用途：官方侧在国内链路会出现「200 后 body 停住」，
/// 单文件下载失败后换镜像再试一次，而不是把整个安装判失败。
pub(crate) fn mirror_url(url: &str) -> Option<String> {
    const PAIRS: [(&str, &str); 4] = [
        (META_OFFICIAL, META_BMCLAPI),
        (META_BMCLAPI, META_OFFICIAL),
        (MAVEN_OFFICIAL, MAVEN_BMCLAPI),
        (MAVEN_BMCLAPI, MAVEN_OFFICIAL),
    ];
    for (from, to) in PAIRS {
        if let Some(rest) = url.strip_prefix(from) {
            return Some(format!("{to}{rest}"));
        }
    }
    None
}

/// Fabric meta 请求的候选源顺序：主选在前，官方 ↔ BMCLAPI 互为兜底。
///
/// 官方 `meta.fabricmc.net` 落在 Cloudflare，国内链路存在「返回 200 后 body
/// 停在半途」的干扰，且只在部分连接上出现；单源请求会直接表现为加载器列表为空
/// 或安装卡住。未识别的地址只返回自身。
pub(crate) fn meta_candidates(primary: &str) -> Vec<String> {
    let counterpart = if let Some(rest) = primary.strip_prefix(META_OFFICIAL) {
        format!("{META_BMCLAPI}{rest}")
    } else if let Some(rest) = primary.strip_prefix(META_BMCLAPI) {
        format!("{META_OFFICIAL}{rest}")
    } else {
        return vec![primary.to_string()];
    };
    vec![primary.to_string(), counterpart]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_base_falls_back_to_bmclapi() {
        assert_eq!(
            meta_candidates("https://meta.fabricmc.net/v2/versions"),
            vec![
                "https://meta.fabricmc.net/v2/versions".to_string(),
                "https://bmclapi2.bangbang93.com/fabric-meta/v2/versions".to_string(),
            ]
        );
    }

    #[test]
    fn bmclapi_base_falls_back_to_official() {
        // 安装器用的是带尾斜杠的根地址，兜底必须保持拼接后的路径同构
        assert_eq!(
            meta_candidates("https://bmclapi2.bangbang93.com/fabric-meta/"),
            vec![
                "https://bmclapi2.bangbang93.com/fabric-meta/".to_string(),
                "https://meta.fabricmc.net/".to_string(),
            ]
        );
    }

    #[test]
    fn unknown_base_has_no_fallback() {
        assert_eq!(
            meta_candidates("https://example.invalid/meta/"),
            vec!["https://example.invalid/meta/".to_string()]
        );
    }

    #[test]
    fn mirror_url_swaps_meta_and_maven_hosts() {
        assert_eq!(
            mirror_url("https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.0/x.jar"),
            Some(
                "https://bmclapi2.bangbang93.com/maven/net/fabricmc/fabric-loader/0.16.0/x.jar"
                    .to_string()
            )
        );
        assert_eq!(
            mirror_url("https://bmclapi2.bangbang93.com/fabric-meta/v2/x"),
            Some("https://meta.fabricmc.net/v2/x".to_string())
        );
        assert_eq!(mirror_url("https://libraries.minecraft.net/a.jar"), None);
    }
}
