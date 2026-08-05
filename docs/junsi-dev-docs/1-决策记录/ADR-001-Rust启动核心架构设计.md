# ADR-001：Rust 启动核心架构设计（rlib 标准架构）

- 日期：2026-08-06
- 状态：已确认
- 场景：将 Qomicex.Core.AOT（.NET 10 Native AOT，164 文件 / ~15,890 行）重构移植为 Rust 启动核心库，供 Qomicex.Tauri 后端消费。
- 方案：
  - 纯库 crate（`crate-type = ["rlib"]`，预留 cdylib），无 bin
  - 三层可见性：`api/` 公开 trait → `core.rs` Facade（`Arc<dyn Trait>` 注入）→ `services/` 全部 `pub(crate)`
  - Builder 模式唯一入口 `GameCore::builder()`，参数集中 `CoreOptions`
  - 异步模型 tokio + reqwest；进度/事件用 `mpsc::channel<CoreEvent>`（弃用闭包回调，规避生命周期问题）
  - 全模型 serde derive，零反射；错误统一 `Error` enum（thiserror）
  - feature flags：default minimal，`expansion`/`java-download`/`lan` opt-in
  - 依赖：serde, serde_json, tokio, reqwest, thiserror, toml, sha1, sha2, base64, zip, bzip2
- 不选方案：
  - 一次性逐字翻译（>5000 行，违反原子化门禁）
  - blocking IO 全同步（与 Tauri 后端异步模型不匹配）
  - 回调闭包做进度报告（生命周期跨线程难管理，AOT 下最易出错）
  - 直接依赖 .NET 生成的 JSON schema（应移植模型结构而非运行时反射）
- 影响范围：整个 src/ 目录，docs/architecture.md，MAPPING_TABLE.yaml，Cargo.toml（依赖将在 B1 引入）
- 移植顺序：依赖 DAG 分 14 批，每批 ≤200 行，批末编译 + 检查点 commit + QA 快照比对
