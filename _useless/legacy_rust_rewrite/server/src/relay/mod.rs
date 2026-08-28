//! relay：从 rrserver 迁入的反向隧道中继能力（阶段 A 原样复用，不重写）。
//!
//! 包含：云端 server（注册 / WS 控制 / `/t/:name/*` 反代，支持 SSE 流式）、
//! 家庭端 client（隧道客户端，逐块回传）、llm_server 部署+注册包装。

pub mod client;
pub mod llmsrv;
pub mod protocol;
pub mod server;
pub mod skill;
pub mod state;
