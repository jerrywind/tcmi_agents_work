//! 中继隧道在 WebSocket 上传输的 JSON 消息协议。
//!
//! 云端 rrserver 与家庭端隧道客户端通过一条持久的 WebSocket 连接通信：
//! - `ServerToClient::Request`  ：云端把外部收到的 HTTP 请求发给家庭端，由其转发到本地 llm 服务。
//! - `ClientToServer::Response` ：家庭端把本地 llm 服务的响应回传云端，再返回给外部调用方。
//! - `Ping` / `Pong`            ：应用层心跳，保持 NAT 后的连接活跃。

use serde::{Deserialize, Serialize};

/// 过滤 HTTP 逐跳（hop-by-hop）头，避免转发 Connection/Upgrade 等控制头。
pub fn is_hop_by_hop_str(k: &str) -> bool {
    matches!(
        k.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// 二进制 body 用 base64 编解码，保证 JSON 文本通道可安全承载任意字节。
mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(b))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let st = String::deserialize(d)?;
        STANDARD.decode(st).map_err(serde::de::Error::custom)
    }
}

/// 云端转发给家庭端的请求。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RequestMsg {
    pub req_id: String,
    #[serde(default = "default_method")]
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(with = "base64_bytes")]
    pub body: Vec<u8>,
}

/// 家庭端回传的响应。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResponseMsg {
    pub req_id: String,
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(with = "base64_bytes")]
    pub body: Vec<u8>,
}

/// 家庭端回传的流式响应分片（用于 LLM 增量输出 / SSE）。
///
/// - 首片携带 `status` 与 `headers`，`chunk` 可为空；
/// - 后续片 `status`/`headers` 被忽略，仅 `chunk` 有效；
/// - 末片 `done = true` 表示流结束（其 `chunk` 仍可能含最后一段数据）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResponseChunkMsg {
    pub req_id: String,
    #[serde(default)]
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(with = "base64_bytes")]
    pub chunk: Vec<u8>,
    pub done: bool,
}

fn default_method() -> String {
    "GET".to_string()
}

/// 云端 → 家庭端：心跳探活请求。
///
/// 转发等待超时、或心跳回收任务扫描到静默服务时由云端主动发出；
/// `probe_id` 用于与回应配对（一次探活一个 id）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HeartbeatProbe {
    pub probe_id: String,
}

/// 家庭端 → 云端：心跳探活回应。
///
/// `alive` 表示本地 llm 服务确实在运行（缺省视为 true，兼容只回 probe_id 的老实现）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HeartbeatAck {
    pub probe_id: String,
    #[serde(default = "default_alive")]
    pub alive: bool,
}

fn default_alive() -> bool {
    true
}

/// 云端 → 家庭端。
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerToClient {
    Request(RequestMsg),
    Ping,
    Heartbeat(HeartbeatProbe),
}

/// 家庭端 → 云端。
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ClientToServer {
    Response(ResponseMsg),
    ResponseChunk(ResponseChunkMsg),
    Pong,
    Heartbeat(HeartbeatAck),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_by_hop_detects_standard_headers() {
        // 标准逐跳头（含大小写不敏感）应被识别
        assert!(is_hop_by_hop_str("Connection"));
        assert!(is_hop_by_hop_str("connection"));
        assert!(is_hop_by_hop_str("Upgrade"));
        assert!(is_hop_by_hop_str("Transfer-Encoding"));
        // 普通头不应被过滤
        assert!(!is_hop_by_hop_str("Content-Type"));
        assert!(!is_hop_by_hop_str("Authorization"));
        assert!(!is_hop_by_hop_str("X-Custom"));
    }

    #[test]
    fn request_round_trip_preserves_binary_body() {
        // body 经 base64 编码，应能无损承载任意字节（含非 UTF-8）
        let req = RequestMsg {
            req_id: "r1".into(),
            method: "POST".into(),
            path: "/v1/chat".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: vec![0u8, 159, 146, 150, 255],
        };
        let s = serde_json::to_string(&req).unwrap();
        // 序列化后的 body 必须是可打印 base64 字符串
        assert!(s.chars().all(|c| !c.is_control()));
        let back: RequestMsg = serde_json::from_str(&s).unwrap();
        assert_eq!(back.req_id, "r1");
        assert_eq!(back.method, "POST");
        assert_eq!(back.path, "/v1/chat");
        assert_eq!(back.body, vec![0u8, 159, 146, 150, 255]);
    }

    #[test]
    fn request_defaults_method_to_get() {
        // 缺省 method 时应回退为 GET
        let json = r#"{"req_id":"x","path":"/ping","headers":[],"body":""}"#;
        let req: RequestMsg = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "GET");
    }

    #[test]
    fn server_to_client_tagged_enums() {
        let s = serde_json::to_string(&ServerToClient::Ping).unwrap();
        assert_eq!(s, r#"{"type":"ping"}"#);
        let back: ServerToClient = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, ServerToClient::Ping));

        let m = ServerToClient::Request(RequestMsg {
            req_id: "r2".into(),
            method: "GET".into(),
            path: "/m".into(),
            headers: vec![],
            body: b"hi".to_vec(),
        });
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains(r#""type":"request""#));
        match serde_json::from_str::<ServerToClient>(&s).unwrap() {
            ServerToClient::Request(r) => assert_eq!(r.body, b"hi"),
            _ => panic!("expected Request variant"),
        }
    }

    #[test]
    fn client_to_server_response_round_trip() {
        let m = ClientToServer::Response(ResponseMsg {
            req_id: "r3".into(),
            status: 200,
            headers: vec![("X-Test".into(), "1".into())],
            body: vec![1, 2, 3],
        });
        let s = serde_json::to_string(&m).unwrap();
        match serde_json::from_str::<ClientToServer>(&s).unwrap() {
            ClientToServer::Response(r) => {
                assert_eq!(r.req_id, "r3");
                assert_eq!(r.status, 200);
                assert_eq!(r.body, vec![1, 2, 3]);
            }
            _ => panic!("expected Response variant"),
        }
    }

    #[test]
    fn response_round_trip_preserves_binary_body() {
        // ResponseMsg 与 RequestMsg 共用 base64 编解码，应同样无损承载非 UTF-8 字节
        let resp = ResponseMsg {
            req_id: "r4".into(),
            status: 418,
            headers: vec![("X-A".into(), "b".into())],
            body: vec![0u8, 255, 254, 1, 2],
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: ResponseMsg = serde_json::from_str(&s).unwrap();
        assert_eq!(back.req_id, "r4");
        assert_eq!(back.status, 418);
        assert_eq!(back.headers, vec![("X-A".into(), "b".into())]);
        assert_eq!(back.body, vec![0u8, 255, 254, 1, 2]);
    }

    #[test]
    fn client_to_server_tags_use_original_case() {
        // ClientToServer 仅含 tag，无 rename_all，故变体名保持原样 "Pong"
        let s = serde_json::to_string(&ClientToServer::Pong).unwrap();
        assert_eq!(s, r#"{"type":"Pong"}"#);
        let back: ClientToServer = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, ClientToServer::Pong));
    }

    #[test]
    fn response_chunk_round_trip_preserves_binary_and_flags() {
        // 流式首片：携带 status/headers，chunk 可为空
        let head = ClientToServer::ResponseChunk(ResponseChunkMsg {
            req_id: "r5".into(),
            status: 200,
            headers: vec![("Content-Type".into(), "text/event-stream".into())],
            chunk: vec![],
            done: false,
        });
        let s = serde_json::to_string(&head).unwrap();
        assert!(s.contains(r#""type":"ResponseChunk""#));
        match serde_json::from_str::<ClientToServer>(&s).unwrap() {
            ClientToServer::ResponseChunk(c) => {
                assert_eq!(c.req_id, "r5");
                assert_eq!(c.status, 200);
                assert!(!c.done);
            }
            _ => panic!("expected ResponseChunk variant"),
        }

        // 流式末片：done=true 且带最后一段数据
        let tail = ClientToServer::ResponseChunk(ResponseChunkMsg {
            req_id: "r5".into(),
            status: 0,
            headers: vec![],
            chunk: vec![65, 66], // "AB"
            done: true,
        });
        let s = serde_json::to_string(&tail).unwrap();
        match serde_json::from_str::<ClientToServer>(&s).unwrap() {
            ClientToServer::ResponseChunk(c) => {
                assert!(c.done);
                assert_eq!(c.chunk, b"AB");
            }
            _ => panic!("expected ResponseChunk variant"),
        }

        // 缺省字段应回退：非首片不传 status/headers 仍可反序列化
        let minimal = r#"{"type":"ResponseChunk","req_id":"r6","chunk":"","done":true}"#;
        let c = serde_json::from_str::<ClientToServer>(minimal).unwrap();
        match c {
            ClientToServer::ResponseChunk(c) => {
                assert_eq!(c.status, 0);
                assert!(c.headers.is_empty());
                assert!(c.done);
            }
            _ => panic!("expected ResponseChunk variant"),
        }
    }

    #[test]
    fn heartbeat_probe_round_trip() {
        // 云端下发探活：lowercase 的 "heartbeat"
        let s = serde_json::to_string(&ServerToClient::Heartbeat(HeartbeatProbe {
            probe_id: "p1".into(),
        }))
        .unwrap();
        assert_eq!(s, r#"{"type":"heartbeat","probe_id":"p1"}"#);
        match serde_json::from_str::<ServerToClient>(&s).unwrap() {
            ServerToClient::Heartbeat(p) => assert_eq!(p.probe_id, "p1"),
            _ => panic!("expected Heartbeat variant"),
        }
    }

    #[test]
    fn heartbeat_ack_round_trip_and_defaults_alive() {
        let s = serde_json::to_string(&ClientToServer::Heartbeat(HeartbeatAck {
            probe_id: "p1".into(),
            alive: false,
        }))
        .unwrap();
        match serde_json::from_str::<ClientToServer>(&s).unwrap() {
            ClientToServer::Heartbeat(a) => {
                assert_eq!(a.probe_id, "p1");
                assert!(!a.alive);
            }
            _ => panic!("expected Heartbeat variant"),
        }

        // 老实现可能只回 probe_id：缺省 alive 视为 true
        let minimal = r#"{"type":"Heartbeat","probe_id":"p2"}"#;
        match serde_json::from_str::<ClientToServer>(minimal).unwrap() {
            ClientToServer::Heartbeat(a) => {
                assert_eq!(a.probe_id, "p2");
                assert!(a.alive, "缺省 alive 应为 true");
            }
            _ => panic!("expected Heartbeat variant"),
        }
    }

    #[test]
    fn invalid_base64_body_fails_to_deserialize() {
        // body 字段如果不是合法 base64，反序列化必须报错而非静默损坏
        let json =
            r#"{"req_id":"x","method":"GET","path":"/p","headers":[],"body":"@@not base64@@"}"#;
        let res = serde_json::from_str::<RequestMsg>(json);
        assert!(res.is_err());
    }

    #[test]
    fn empty_body_round_trips_as_empty_base64() {
        // 空 body 编码为空字符串，解码应回到空字节
        let req = RequestMsg {
            req_id: "e".into(),
            method: "GET".into(),
            path: "/".into(),
            headers: vec![],
            body: vec![],
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: RequestMsg = serde_json::from_str(&s).unwrap();
        assert!(back.body.is_empty());
    }
}
