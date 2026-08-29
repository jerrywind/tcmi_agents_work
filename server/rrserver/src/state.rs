//! 隧道注册表：维护「隧道名 → 控制通道发送端」与「请求 id → 等待响应」的映射。
//!
//! 云端 rrserver 进程内唯一。家庭端通过 WebSocket 连上后把它的发送端注册进来；
//! 外部请求到达时，server 生成一个 req_id、登记等待、再把 `Request` 命令发给对应隧道，
//! 待家庭端回传 `Response` 后通过 oneshot 唤醒等待方。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, Mutex};

use crate::protocol::{RequestMsg, ResponseChunkMsg, ResponseMsg};

/// 发给家庭端的控制命令。
pub enum TunnelCommand {
    Request(RequestMsg),
}

/// 等待中的响应。
pub struct PendingResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

type TunnelTx = mpsc::UnboundedSender<TunnelCommand>;
type ChunkTx = mpsc::UnboundedSender<ResponseChunkMsg>;

#[derive(Clone)]
pub struct Registry {
    tunnels: Arc<Mutex<HashMap<String, TunnelTx>>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<PendingResponse>>>>,
    /// 流式响应通道：请求 id → 家庭端回传的分片发送端。
    streams: Arc<Mutex<HashMap<String, ChunkTx>>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            tunnels: Arc::new(Mutex::new(HashMap::new())),
            pending: Arc::new(Mutex::new(HashMap::new())),
            streams: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 同步清理某个请求遗留的等待项与流式通道。
    ///
    /// 供 `Stream::poll_next` 这类**不可 await** 的上下文使用。
    ///
    /// ⚠️ 切勿写成 `let _ = self.cancel_pending(id);`——`cancel_pending` 是 `async fn`，
    /// 那只创建 future 随即丢弃，清理**永远不会执行**
    /// （clippy: `non-binding let on a future`），会导致 `pending` / `streams` 泄漏。
    ///
    /// 临界区极短（仅 `HashMap::remove`，内部无 await），故 `try_lock` 基本必然成功。
    /// 清理是幂等的。
    pub fn cleanup_pending_sync(&self, req_id: &str) {
        if let Ok(mut p) = self.pending.try_lock() {
            p.remove(req_id);
        }
        if let Ok(mut s) = self.streams.try_lock() {
            s.remove(req_id);
        }
    }

    /// 家庭端 WS 建立后注册其发送端。
    pub async fn register(&self, name: &str, tx: TunnelTx) {
        self.tunnels.lock().await.insert(name.to_string(), tx);
    }

    /// 断开后移除。
    pub async fn unregister(&self, name: &str) {
        self.tunnels.lock().await.remove(name);
    }

    /// 仅当注册表中该名字当前对应的仍是 `tx` 这条通道时才移除。
    ///
    /// 用于连接退出时的清理：若同名**新连接**已注册（`register` 的 insert 会替换旧项），
    /// 旧连接的延迟清理绝不能误删新隧道——否则会出现「重连成功却被旧连接的
    /// 收尾逻辑踢下线」的竞态（半开连接场景下尤其常见）。
    pub async fn unregister_if_same(&self, name: &str, tx: &TunnelTx) {
        let mut map = self.tunnels.lock().await;
        if let Some(cur) = map.get(name) {
            if cur.same_channel(tx) {
                map.remove(name);
            }
        }
    }

    /// 向外转发一个请求；若无该隧道返回 false。
    pub async fn send_request(&self, name: &str, req: RequestMsg) -> bool {
        let tx = self.tunnels.lock().await.get(name).cloned();
        match tx {
            Some(tx) => tx.send(TunnelCommand::Request(req)).is_ok(),
            None => false,
        }
    }

    /// 登记一个等待响应的接收端，返回 oneshot Receiver。
    pub async fn new_pending(&self, req_id: &str) -> oneshot::Receiver<PendingResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(req_id.to_string(), tx);
        rx
    }

    /// 响应到达后唤醒等待方，并清理可能打开的流式通道（完整响应与流二选一）。
    pub async fn resolve(&self, resp: ResponseMsg) {
        if let Some(tx) = self.pending.lock().await.remove(&resp.req_id) {
            let _ = tx.send(PendingResponse {
                status: resp.status,
                headers: resp.headers,
                body: resp.body,
            });
        }
        // 注意：这里**不要**清理 `streams` 中的通道。完整响应路径不会经由 `push_chunk`，
        // 若在此 drop 流发送端，`proxy_handler` 里 `select!` 会误把 `chunk_rx.recv()` 的
        // 立即 `None` 当作「流已关闭」而返回 502，与本次完整响应形成竞态。
        // 流的清理交由 `cancel_stream`（见 `proxy_handler` 的 `Outcome::Full` 分支）。
    }

    /// 请求超时/发送失败时清理登记（同时清理流式通道）。
    pub async fn cancel_pending(&self, req_id: &str) {
        self.pending.lock().await.remove(req_id);
        self.streams.lock().await.remove(req_id);
    }

    /// 打开该请求的流式通道，返回分片接收端。
    /// 家庭端若以流式回传，server 侧 `proxy_handler` 会消费此通道拼装响应体。
    pub async fn open_stream(&self, req_id: &str) -> mpsc::UnboundedReceiver<ResponseChunkMsg> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.streams.lock().await.insert(req_id.to_string(), tx);
        rx
    }

    /// 同步清理某个请求的流式通道（语义同上，只清 `streams`）。
    pub fn cleanup_stream_sync(&self, req_id: &str) {
        if let Ok(mut s) = self.streams.try_lock() {
            s.remove(req_id);
        }
    }

    /// 家庭端回传一个响应分片。若是末片（`done=true`），清理流式通道；
    /// 但**不要**在这里移除 `pending`（oneshot）——流式路径下 oneshot 不会被 `resolve` 使用，
    /// 其清理交由 `ChunkStream` 结束时调用 `cancel_pending`，否则会与 `proxy_handler` 的
    /// `select!` 竞态：末片 `remove` 掉 oneshot 发送端会让 `rx.recv()` 瞬间变 `None`，
    /// 被 `select!` 误当作「完整响应已到达」返回 502。
    pub async fn push_chunk(&self, chunk: ResponseChunkMsg) {
        let req_id = chunk.req_id.clone();
        let done = chunk.done;
        if let Some(tx) = self.streams.lock().await.get(&req_id) {
            let _ = tx.send(chunk);
        }
        if done {
            self.streams.lock().await.remove(&req_id);
        }
    }

    /// 显式关闭流式通道（超时等场景下调用）。
    pub async fn cancel_stream(&self, req_id: &str) {
        self.streams.lock().await.remove(req_id);
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn sample_req(req_id: &str) -> RequestMsg {
        RequestMsg {
            req_id: req_id.to_string(),
            method: "GET".into(),
            path: "/x".into(),
            headers: vec![],
            body: vec![],
        }
    }

    #[tokio::test]
    async fn register_then_send_then_resolve_awakens_waiter() {
        let reg = Registry::new();
        let (tx, mut rx) = mpsc::unbounded_channel::<TunnelCommand>();
        reg.register("home", tx).await;

        // pending 必须在 send 之前登记，否则 resolve 时找不到等待方
        let mut prx = reg.new_pending("r1").await;
        assert!(reg.send_request("home", sample_req("r1")).await);

        // 从隧道控制通道取出下发的命令
        let cmd = rx.recv().await.expect("command should be delivered");
        match cmd {
            TunnelCommand::Request(r) => {
                assert_eq!(r.req_id, "r1");
                assert_eq!(r.path, "/x");
                reg.resolve(ResponseMsg {
                    req_id: r.req_id,
                    status: 200,
                    headers: vec![],
                    body: b"ok".to_vec(),
                })
                .await;
            }
        }

        let pending = prx.try_recv().expect("waiter should be woken");
        assert_eq!(pending.status, 200);
        assert_eq!(pending.body, b"ok");
    }

    #[tokio::test]
    async fn send_request_to_unknown_tunnel_returns_false() {
        let reg = Registry::new();
        assert!(!reg.send_request("nope", sample_req("z")).await);
    }

    #[tokio::test]
    async fn unregister_removes_tunnel() {
        let reg = Registry::new();
        let (tx, _rx) = mpsc::unbounded_channel::<TunnelCommand>();
        reg.register("home", tx).await;
        reg.unregister("home").await;
        assert!(!reg.send_request("home", sample_req("a")).await);
    }

    #[tokio::test]
    async fn unregister_if_same_only_removes_own_registration() {
        let reg = Registry::new();
        let (old_tx, _old_rx) = mpsc::unbounded_channel::<TunnelCommand>();
        let (new_tx, mut new_rx) = mpsc::unbounded_channel::<TunnelCommand>();

        // 旧连接注册后，被同名新连接替换（insert 覆盖）
        reg.register("home", old_tx.clone()).await;
        reg.register("home", new_tx.clone()).await;

        // 旧连接退出清理：不得误删新隧道
        reg.unregister_if_same("home", &old_tx).await;
        assert!(reg.send_request("home", sample_req("a")).await);
        assert!(new_rx.recv().await.is_some());

        // 新连接自身退出清理：应正常移除
        reg.unregister_if_same("home", &new_tx).await;
        assert!(!reg.send_request("home", sample_req("b")).await);
    }

    #[tokio::test]
    async fn resolve_without_pending_is_noop() {
        // 响应到达但无等待方，不应 panic
        let reg = Registry::new();
        reg.resolve(ResponseMsg {
            req_id: "missing".into(),
            status: 200,
            headers: vec![],
            body: vec![],
        })
        .await;
    }

    #[tokio::test]
    async fn cancel_pending_clears_waiter() {
        let reg = Registry::new();
        let mut prx = reg.new_pending("r9").await;
        reg.cancel_pending("r9").await;
        // 取消后 resolve 不会再投递，等待方立即得到 None
        assert!(prx.try_recv().is_err());
    }

    #[tokio::test]
    async fn send_request_to_closed_channel_returns_false() {
        // 注册后，若家庭端 WS 断开（发送端被 drop），send 会失败，应返回 false
        let reg = Registry::new();
        let (tx, rx) = mpsc::unbounded_channel::<TunnelCommand>();
        reg.register("home", tx).await;
        drop(rx); // 模拟对端断开
        assert!(!reg.send_request("home", sample_req("r1")).await);
    }

    #[tokio::test]
    async fn registry_is_safe_under_concurrent_access() {
        // 多个隧道并发注册/发送/注销，不应 panic 或丢失数据
        use std::sync::Arc;

        let reg = Arc::new(Registry::new());
        let mut handles = Vec::new();
        for i in 0..8u32 {
            let reg = reg.clone();
            handles.push(tokio::spawn(async move {
                let name = format!("t{}", i);
                let (tx, _rx) = mpsc::unbounded_channel::<TunnelCommand>();
                reg.register(&name, tx).await;
                // 注册后通道存活，send 入队成功应返回 true
                assert!(reg.send_request(&name, sample_req("x")).await);
                reg.unregister(&name).await;
                // unregister 后发送端已丢弃，send 应返回 false
                assert!(!reg.send_request(&name, sample_req("y")).await);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn reregister_overwrites_previous_tunnel() {
        // 同名隧道重新注册应覆盖旧的发送端，而非叠加
        let reg = Registry::new();
        let (tx1, rx1) = mpsc::unbounded_channel::<TunnelCommand>();
        let (tx2, _rx2) = mpsc::unbounded_channel::<TunnelCommand>();
        reg.register("home", tx1).await;
        reg.register("home", tx2).await;
        drop(rx1); // 旧通道已断开
                   // 应能经新通道成功下发命令
        assert!(reg.send_request("home", sample_req("r1")).await);
    }

    #[tokio::test]
    async fn stream_delivers_chunks_and_closes_on_done() {
        // 家庭端流式回传：首片 + 数据片 + 末片，server 应能按序收到并在 done 后关闭
        let reg = Registry::new();
        let mut rx = reg.open_stream("s1").await;
        reg.push_chunk(ResponseChunkMsg {
            req_id: "s1".into(),
            status: 200,
            headers: vec![],
            chunk: vec![],
            done: false,
        })
        .await;
        reg.push_chunk(ResponseChunkMsg {
            req_id: "s1".into(),
            status: 0,
            headers: vec![],
            chunk: b"AB".to_vec(),
            done: false,
        })
        .await;
        // 此时通道仍存活（未 done）
        assert!(!rx.is_empty());
        reg.push_chunk(ResponseChunkMsg {
            req_id: "s1".into(),
            status: 0,
            headers: vec![],
            chunk: b"CD".to_vec(),
            done: true,
        })
        .await;
        // done 后发送端已从 map 移除；接收顺序：首片(空) -> AB -> CD(done)
        let head = rx.recv().await.unwrap();
        assert!(!head.done);
        assert!(head.chunk.is_empty());
        let c1 = rx.recv().await.unwrap();
        assert_eq!(c1.chunk, b"AB");
        let c2 = rx.recv().await.unwrap();
        assert!(c2.done);
        assert_eq!(c2.chunk, b"CD");
        // 通道已关闭，后续不再有数据
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn resolve_clears_open_stream() {
        // 完整响应到达时，应同时清理可能打开的流式通道，避免泄漏
        let reg = Registry::new();
        let _rx = reg.open_stream("s2").await;
        reg.resolve(ResponseMsg {
            req_id: "s2".into(),
            status: 200,
            headers: vec![],
            body: b"done".to_vec(),
        })
        .await;
        // 再次打开应为全新通道（旧 sender 已被移除）
        let mut rx = reg.open_stream("s2").await;
        reg.push_chunk(ResponseChunkMsg {
            req_id: "s2".into(),
            status: 0,
            headers: vec![],
            chunk: b"x".to_vec(),
            done: true,
        })
        .await;
        assert_eq!(rx.recv().await.unwrap().chunk, b"x");
    }
}
