//! Java 域实现：扫描 / 推荐 / 下载 / 聚合
// 过渡期：builder 组装（P22）完成前允许未使用告警；组装后移除本行
#![allow(dead_code)]
pub mod scanner;
pub mod recommend;
pub mod download;
pub mod provider;
