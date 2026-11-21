//! minio-rs 的纯逻辑核心：菜单树、导航与后续的帧解析（NMEA/802.15.4/pcap）。
//! 零依赖、始终 no_std，宿主 `cargo test` 原生可跑。
#![no_std]

pub mod menu;
pub mod lease;
pub mod nmea;
pub mod config;
pub mod pcap;
pub mod dns;
pub mod frame154;
pub mod trackers;
