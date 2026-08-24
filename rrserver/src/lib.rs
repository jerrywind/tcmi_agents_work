//! rrserver 库：云端中继服务器与家庭端隧道客户端的可复用实现。
//!
//! 本 crate 同时提供二进制（`rrserver` CLI）与库接口，便于集成测试直接驱动内部组件。

pub mod client;
pub mod llmsrv;
pub mod protocol;
pub mod server;
pub mod skill;
pub mod state;
