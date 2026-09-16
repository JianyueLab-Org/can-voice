//! 给人看的话，在 Rust 侧只是一个字典 key（#29）。
//!
//! # 措辞在前端的字典里，不在这里
//!
//! 四个客户端的界面要能在中文和英文之间切换，而且是**当场切**。Rust 侧拼好一句
//! 中文交出去的话，切到英文之后那一句还是中文。旧版 can-audio 让下层自己调 `t()`，
//! 那是因为它们和界面在同一个进程里读同一个语言设置；这里的下层在 Rust，语言设置
//! 和字典在 webview 里。所以下层交出去的是 [`Message`]：一个 key 加上要填进去的
//! 值，前端的 `errorText()` 照当前语言翻。
//!
//! # 规矩
//!
//! - key 写成字面量：`Message::new("error.log.credentials")`。`tests/dictionaries.rs`
//!   扫源码里的这些字面量，每一个都得在字典里——手改字典时删掉一段 key 的事，
//!   旧版真出过。
//! - 共用 crate 发的 key 放在四个客户端逐字节相同的 `common` 字典里；只有通播
//!   用的 crate（`can-voice-atis`）发的放在通播自己的字典里。
//! - 值是原样的数据：频率、地址、HTTP 状态码、底层的英文错误。**不是半句翻译好
//!   的话**——那半句切了语言也不会跟着变。
//! - 日志照旧是英文。[`Message`] 的 `Display` 只打 key 和值，是给看日志的人的。

use std::collections::BTreeMap;

/// 一句还没翻译的话。
///
/// 序列化成 `{ "key": "...", "values": { ... } }`，Tauri 命令把它原样当错误交给
/// 前端。没有值时不带 `values`。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Message {
    pub key: &'static str,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<&'static str, String>,
}

impl Message {
    pub fn new(key: &'static str) -> Self {
        Self {
            key,
            values: BTreeMap::new(),
        }
    }

    /// 填一个占位符。`name` 对应字典里的 `{name}`。
    pub fn with(mut self, name: &'static str, value: impl std::fmt::Display) -> Self {
        self.values.insert(name, value.to_string());
        self
    }
}

impl std::fmt::Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key)?;
        for (name, value) in &self.values {
            write!(f, " {name}={value:?}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Message {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 前端 `errorText()` 认的就是这个形状。
    #[test]
    fn a_message_reaches_the_frontend_as_a_key_and_its_values() {
        let m = Message::new("error.log.rejected").with("status", 502);
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({ "key": "error.log.rejected", "values": { "status": "502" } })
        );
        assert_eq!(
            serde_json::to_value(Message::new("error.log.no_file")).unwrap(),
            serde_json::json!({ "key": "error.log.no_file" })
        );
    }

    /// 进日志的那一份是英文字符的 key 加上值，不需要翻译。
    #[test]
    fn the_log_form_names_the_key_and_the_values() {
        let m = Message::new("error.endpoint.url_scheme.api_origin").with("value", "api.example");
        assert_eq!(
            m.to_string(),
            r#"error.endpoint.url_scheme.api_origin value="api.example""#
        );
    }
}
