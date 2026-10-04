//! JarMod 扩展点（issue #180）。
//!
//! ⚠️ TODO(#180)：JarMod 支持目前未实现——本期（#123 期1，Technic SingleZip）
//! 仅识别 Technic 包内的 loader 与 MC 版本，modpack.jar 本体不注入游戏。
//! 对 `bin/modpack.jar` 内**不含** `version.json` 的古董包（≈1.4.x 及更早），
//! 上层（`qomicex-backend` technic 转换器）必须拒绝导入并报
//! `TECHNIC_JARMOD_UNSUPPORTED`（见 `endpoints/modpack.rs` technic 模块）。
//!
//! 实现本能力时在此模块落地（候选语义，实施前对照 Prism `installJarMods` 的
//! MMC 组件 patch 落盘格式与 HMCL 等价实现定稿）：
//! - a) 启动前把 jarmod 内容合并进主 jar（需缓存/还原原 jar，与校验/重装交互）；
//! - b) classpath 前置注入（非破坏，资源加载顺序敏感的老 mod 可能有行为差异）；
//! - c) 对齐 HMCL/MultiMC：MMC patch `jarmods` 字段 + 版本构建层展开。
//!
//! 届时应提供：
//! - 版本 JSON schema 的 jarmod 扩展字段与解析；
//! - 启动链路对 jarmod 的展开（合并或注入）；
//! - 错误类型：jarmod 文件缺失 / 非法 jar / 合并失败。

/// 占位常量：jarmod 能力未实现。实现 #180 时移除或改为特性探测。
pub const JARMOD_UNSUPPORTED: bool = true;

#[cfg(test)]
mod tests {
    #[test]
    fn jarmod_placeholder_is_unsupported() {
        // 占位接口：#180 实现后替换。防止无人维护的静默腐烂。
        assert!(super::JARMOD_UNSUPPORTED);
    }
}
