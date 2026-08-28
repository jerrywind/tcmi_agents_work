//! 隧道注册表（与 rrserver/src/state.rs 一致）。
//!
//! 维护 `name -> Tunnel { tx, created }`，云端 server 据此向家庭端转发请求，
//! 并做 name 规范化与上限控制。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::ws::Message as WsMessage;
use tokio::sync::mpsc;

use super::protocol::{Chunk, Frame, Request};

/// 一次隧道转发请求的等待者：首部通过 oneshot 回传，后续 Chunk 存入缓冲队列。
struct Waiter {
    /// 首部（Response）回传通道；消费后即置 None。
    header: Option<tokio::sync::oneshot::Sender<Frame>>,
    /// 已到达但未取出的 Chunk 缓冲（流式）。
    chunks: Vec<Chunk>,
    /// 是否已收到 done。
    done: bool,
}

/// 转发协调器：rid -> Waiter。云端把请求发给家庭端后，家庭端回传的
/// Response/Chunk 通过 `deliver` 入队，转发逻辑用 `register_waiter` / `next_chunk` 取出。
pub struct ForwardCoordinator {
    waiters: Mutex<HashMap<String, Waiter>>,
}

impl ForwardCoordinator {
    pub fn new() -> Arc<Self> {
        Arc::new(ForwardCoordinator {
            waiters: Mutex::new(HashMap::new()),
        })
    }

    /// 注册一次转发的等待者，返回首部 oneshot 接收端。
    pub fn register_waiter(&self, rid: &str) -> tokio::sync::oneshot::Receiver<Frame> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.waiters.lock().unwrap().insert(
            rid.to_string(),
            Waiter {
                header: Some(tx),
                chunks: Vec::new(),
                done: false,
            },
        );
        rx
    }

    pub fn unregister_waiter(&self, rid: &str) {
        self.waiters.lock().unwrap().remove(rid);
    }

    /// 家庭端回传帧入队（Response 或 Chunk）。若已是最后一个 Chunk 则标记 done。
    pub fn deliver(&self, frame: Frame) {
        let rid = frame.rid().to_string();
        let mut g = self.waiters.lock().unwrap();
        let w = match g.get_mut(&rid) {
            Some(w) => w,
            None => return,
        };
        match frame {
            Frame::Response(_) => {
                if let Some(tx) = w.header.take() {
                    let _ = tx.send(frame);
                }
            }
            Frame::Chunk(c) => {
                if c.done {
                    w.done = true;
                }
                w.chunks.push(c);
            }
            _ => {}
        }
    }

    /// 取出下一帧 Chunk；若已 done 且缓冲空则返回 None（表示流结束）。
    pub fn next_chunk(&self, rid: &str) -> Option<Chunk> {
        let mut g = self.waiters.lock().unwrap();
        let w = match g.get_mut(rid) {
            Some(w) => w,
            None => return None,
        };
        if let Some(c) = w.chunks.pop() {
            return Some(c);
        }
        if w.done {
            g.remove(rid);
            None
        } else {
            None
        }
    }
}

/// 单条隧道：持有发往家庭端 client 的 mpsc 发送端。
#[derive(Clone)]
pub struct Tunnel {
    pub tx: mpsc::UnboundedSender<WsMessage>,
    pub created: chrono_like::Instant,
}

/// 注册表：name -> Tunnel。用 Arc<Registry> 共享给路由与 WS 处理器。
pub struct Registry {
    map: Mutex<HashMap<String, Tunnel>>,
    pub max: usize,
}

impl Registry {
    pub fn new() -> Arc<Self> {
        Arc::new(Registry {
            map: Mutex::new(HashMap::new()),
            max: 256,
        })
    }

    /// 规范化 name：小写、去空格、限长 63。
    pub fn normalize(name: &str) -> String {
        let mut s: String = name
            .chars()
            .map(|c| if c.is_whitespace() { '-' } else { c })
            .flat_map(|c| c.to_lowercase())
            .collect();
        if s.len() > 63 {
            s.truncate(63);
        }
        s
    }

    /// 注册（若超 max 或已存在则失败）。
    pub fn register(&self, name: &str, tx: mpsc::UnboundedSender<WsMessage>) -> bool {
        let n = Self::normalize(name);
        let mut map = self.map.lock().unwrap();
        if map.len() >= self.max || map.contains_key(&n) {
            return false;
        }
        map.insert(
            n,
            Tunnel {
                tx,
                created: chrono_like::Instant::now(),
            },
        );
        true
    }

    pub fn unregister(&self, name: &str) {
        self.map.lock().unwrap().remove(&Self::normalize(name));
    }

    pub fn get(&self, name: &str) -> Option<Tunnel> {
        self.map.lock().unwrap().get(&Self::normalize(name)).cloned()
    }

    pub fn count(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

/// 轻量时间占位（避免引入 chrono 依赖，仅用于 created 时间戳）。
pub mod chrono_like {
    #[derive(Clone, Copy)]
    pub struct Instant {
        pub secs: u64,
    }
    impl Instant {
        pub fn now() -> Self {
            Instant {
                secs: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            }
        }
    }
}

/// 供转发使用：把 Request 发送进隧道。
pub fn send_request(tunnel: &Tunnel, req: Request) -> bool {
    use super::protocol::Frame;
    let msg = WsMessage::Text(
        serde_json::to_string(&Frame::Request(req)).unwrap_or_default(),
    );
    tunnel.tx.send(msg).is_ok()
}
