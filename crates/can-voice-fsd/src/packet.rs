//! FSD 包的拼装与解析。**纯函数，没有 socket。**
//!
//! 逐条对照 can-fsd 的解析代码（`internal/fsd/conn.go`、`handler.go`、
//! `metar.go`）和 `docs/protocol.md`：
//!
//! ```text
//! 登录     $ID{呼号}:SERVER:{客户端ID}:{客户端名}:{主版本}:{次版本}:{CID}:{机器码}
//!          #AA{呼号}:SERVER:{真实姓名}:{CID}:{密码}:{等级}:{协议版本}
//! 位置     %{呼号}:{频率}:{席位类型}:{可视范围}:{等级}:{纬度}:{经度}:0
//! 通播回复 $CR{呼号}:{对方}:ATIS:T:{一行}  …  末行 :E:{行数}
//! 气象     $AX{呼号}:SERVER:METAR:{ICAO}  →  $ARserver:{呼号}:METAR:{报文}
//! 下线     #DA{呼号}:{CID}
//! ```
//!
//! `$ID` 的第 9 个字段（challenge）**故意留空**：填了服务端就会发起 VATSIM
//! 客户端质询（`$ZC`），那套算法只有官方客户端有密钥表。can-fsd 允许不参与质询
//! （`internal/fsd/conn.go` 的 `authenticate`）。

/// 默认端口。
pub const DEFAULT_PORT: u16 = 6809;
/// `ProtoRevisionClassic`。
pub const PROTO_REVISION: u32 = 100;
/// 席位类型：通播。
pub const FACILITY_ATIS: u32 = 7;
/// 通播不需要管制权限，用最低等级登录一定能通过。
pub const RATING_OBSERVER: u32 = 1;
/// can-fsd 每个席位最多收 64 行。
pub const MAX_ATIS_LINES: usize = 64;
/// 通播文字折行宽度。
pub const ATIS_LINE_WIDTH: usize = 70;
/// can-fsd 的 `MaxCallsignLength`。
///
/// 原来是 10，正好卡死 vATIS 的分离通播 `ZSPD_D_ATIS` / `ZSPD_A_ATIS`
/// （11 个字符），服务端已经放宽到 12。
pub const MAX_CALLSIGN_LENGTH: usize = 12;

pub const CLIENT_ID: &str = "0001";
pub const CLIENT_NAME: &str = "Cerulean Aviation Network ATIS";
pub const CLIENT_MAJOR: u32 = 1;
pub const CLIENT_MINOR: u32 = 0;

const CALLSIGN_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";

/// 呼号不合服务端规矩时说明是哪一条。
///
/// 规则来自 can-fsd 的 `IsValidCallsign` / `IsATISCallsign`。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallsignProblem {
    #[error("{callsign} is {len} characters; it must be between 2 and {limit}")]
    Length {
        callsign: String,
        len: usize,
        limit: usize,
    },
    #[error("{0} contains a character the server will not accept")]
    Charset(String),
    #[error("{0} does not end in _ATIS")]
    NotAtis(String),
}

/// 频率编码：`118.000` → `"18000"`（开头的 1 和小数点是协议隐含的）。
///
/// 认不出来给 `None`。发一个错的频率出去比不发更糟——席位会挂在一个没人听的
/// 频率上，而在线列表里它看着一切正常。
pub fn encode_frequency(frequency: &str) -> Option<String> {
    let mhz: f64 = frequency.trim().parse().ok()?;
    let khz = (mhz * 1000.0).round();
    if !(0.0..=999_999.0).contains(&khz) {
        return None;
    }
    let padded = format!("{:06}", khz as u32);
    Some(padded[1..].to_string())
}

/// 包是冒号分隔的，正文里的冒号和换行会破坏分帧。
pub fn sanitize_line(line: &str) -> String {
    line.replace([':', '\r', '\n'], " ").trim().to_string()
}

/// 把通播文字折成若干行。
pub fn wrap_atis_text(text: &str) -> Vec<String> {
    wrap_atis_text_to(text, ATIS_LINE_WIDTH)
}

pub fn wrap_atis_text_to(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in sanitize_line(text).split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines.truncate(MAX_ATIS_LINES);
    lines
}

/// 呼号合规就返回 `Ok(大写形式)`。
pub fn check_atis_callsign(callsign: &str) -> Result<String, CallsignProblem> {
    let callsign = callsign.trim().to_uppercase();
    let len = callsign.chars().count();
    if !(2..=MAX_CALLSIGN_LENGTH).contains(&len) {
        return Err(CallsignProblem::Length {
            callsign,
            len,
            limit: MAX_CALLSIGN_LENGTH,
        });
    }
    if !callsign.chars().all(|c| CALLSIGN_CHARS.contains(c)) {
        return Err(CallsignProblem::Charset(callsign));
    }
    if !callsign.ends_with("_ATIS") {
        return Err(CallsignProblem::NotAtis(callsign));
    }
    Ok(callsign)
}

/// 机器码。照搬 Python 版的算法——服务端不校验它，但换个算法没有好处。
pub fn machine_id(callsign: &str) -> u64 {
    callsign.chars().map(|c| c as u64).sum::<u64>() * 7919
}

/// 一条 FSD 连接的身份，拼包时反复用到的那几样。
#[derive(Debug, Clone)]
pub struct Identity {
    pub callsign: String,
    pub cid: String,
    pub password: String,
    pub real_name: String,
    pub rating: u32,
}

impl Identity {
    /// **每一格都过 `sanitize_line`，密码也不例外。**
    ///
    /// 带冒号的密码会让 `#AA` 后面的字段全部错位（服务器拒收），而
    /// [`redact`] 按固定下标打码，错位之后冒号后那一截密码会**原样进日志**。
    /// 冒号在 FSD 协议里本来就带不动，所以换成空格是唯一一条既安全又无损的路。
    pub fn new(callsign: &str, cid: &str, password: &str, real_name: &str, rating: u32) -> Self {
        let real_name = sanitize_line(real_name);
        Self {
            callsign: callsign.trim().to_uppercase(),
            cid: sanitize_line(cid),
            password: sanitize_line(password),
            real_name: if real_name.is_empty() {
                "ATIS".to_string()
            } else {
                real_name
            },
            rating,
        }
    }

    /// `$ID` —— 客户端自报家门。第 9 格（challenge）**故意留空**。
    pub fn id_packet(&self) -> String {
        format!(
            "$ID{}:SERVER:{CLIENT_ID}:{CLIENT_NAME}:{CLIENT_MAJOR}:{CLIENT_MINOR}:{}:{}",
            self.callsign,
            self.cid,
            machine_id(&self.callsign)
        )
    }

    /// `#AA` —— 登录。
    pub fn login_packet(&self) -> String {
        format!(
            "#AA{}:SERVER:{}:{}:{}:{}:{PROTO_REVISION}",
            self.callsign, self.real_name, self.cid, self.password, self.rating
        )
    }

    /// `$CQ…CAPS` —— **登录没有专门的成功包**，用一次能力查询换个明确回应。
    pub fn caps_query(&self) -> String {
        format!("$CQ{}:SERVER:CAPS", self.callsign)
    }

    /// `#DA` —— 下线。
    pub fn logoff_packet(&self) -> String {
        format!("#DA{}:{}", self.callsign, self.cid)
    }
}

/// 席位在雷达上的位置和频率。
#[derive(Debug, Clone)]
pub struct Position {
    pub frequency: String,
    pub facility: u32,
    pub vis_range: u32,
    pub rating: u32,
    pub latitude: f64,
    pub longitude: f64,
}

impl Position {
    /// `%` —— 位置包。服务端 150 秒收不到就断线。
    ///
    /// 频率认不出来时返回 `None`，由调用方报错并停掉这条连接。
    pub fn packet(&self, callsign: &str) -> Option<String> {
        let frequency = encode_frequency(&self.frequency)?;
        Some(format!(
            "%{callsign}:{frequency}:{}:{}:{}:{:.5}:{:.5}:0",
            self.facility, self.vis_range, self.rating, self.latitude, self.longitude
        ))
    }
}

/// 回答一次通播查询的那几行。
///
/// 飞行员客户端问的是 `ATIS`；服务端自己的轮询在旧 Python 服务端上收的是
/// `TEXTATIS`（见 can-fsd `handler.go` 的 `handleTextATISResponse`），所以
/// **回给 `SERVER` 时两种都发一遍**，多余的那份会被忽略。
pub fn atis_reply(callsign: &str, recipient: &str, lines: &[String]) -> Vec<String> {
    let kinds: &[&str] = if recipient == "SERVER" {
        &["ATIS", "TEXTATIS"]
    } else {
        &["ATIS"]
    };
    let mut out = Vec::with_capacity(kinds.len() * (lines.len() + 1));
    for kind in kinds {
        for line in lines {
            out.push(format!("$CR{callsign}:{recipient}:{kind}:T:{line}"));
        }
        out.push(format!(
            "$CR{callsign}:{recipient}:{kind}:E:{}",
            lines.len()
        ));
    }
    out
}

/// `$AX` —— 向服务端要一份 METAR。
///
/// 服务端自己有气象源和缓存（can-fsd `internal/fsd/metar.go`），走这条路
/// 就不用再去连外部气象接口。
pub fn metar_request(callsign: &str, icao: &str) -> String {
    format!("$AX{callsign}:SERVER:METAR:{}", icao.trim().to_uppercase())
}

/// `#AA` 的第 5 段是密码，日志里换成星号。
pub fn redact(packet: &str) -> String {
    if !packet.starts_with("#AA") {
        return packet.to_string();
    }
    let mut fields: Vec<&str> = packet.split(':').collect();
    if fields.len() > 4 {
        fields[4] = "***";
    }
    fields.join(":")
}

/// 收到的包里**这一侧关心的那几种**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// `$ER` —— 服务端报错。登录前是致命的，登录后多半只是某次查询失败。
    Error { code: String, message: String },
    /// `$AR…METAR` —— 一份气象报文。
    Metar(String),
    /// `$CQ…ATIS` —— 有人问通播。
    AtisQuery { sender: String },
    /// `$CQ…CAPS` —— 有人问能力。
    CapsQuery { sender: String },
    /// `$CR…CAPS` —— 服务端答了我们的能力查询，**这就是登录成功的确认**。
    CapsReply,
    /// `$PI` —— ping，要回 `$PO`。
    Ping { sender: String, rest: String },
    /// `#TM` —— 服务端的文字消息。
    TextMessage(String),
    /// `#DL` —— 服务端心跳。
    Heartbeat,
    /// 别的都不关心。
    Other,
}

/// 解析一个收到的包。`callsign` 是自己的呼号，用来判断有些包是不是发给自己的。
pub fn parse(packet: &str, callsign: &str) -> Incoming {
    let fields: Vec<&str> = packet.split(':').collect();
    let head = fields[0];

    if let Some(rest) = head.strip_prefix("$ER") {
        let _ = rest;
        return Incoming::Error {
            code: fields.get(2).unwrap_or(&"?").to_string(),
            message: fields
                .get(4)
                .map(|s| s.to_string())
                .unwrap_or_else(|| packet.to_string()),
        };
    }

    if head.starts_with("$AR") && fields.len() >= 4 && fields[2] == "METAR" {
        return Incoming::Metar(fields[3..].join(":").trim().to_string());
    }

    if let Some(sender) = head.strip_prefix("$CQ") {
        if fields.len() >= 3 && fields[1] == callsign {
            return match fields[2] {
                "ATIS" => Incoming::AtisQuery {
                    sender: sender.to_string(),
                },
                "CAPS" => Incoming::CapsQuery {
                    sender: sender.to_string(),
                },
                _ => Incoming::Other,
            };
        }
        return Incoming::Other;
    }

    if head.starts_with("$CR") && fields.len() >= 3 && fields[1] == callsign && fields[2] == "CAPS"
    {
        return Incoming::CapsReply;
    }

    if let Some(sender) = head.strip_prefix("$PI") {
        if fields.len() >= 3 {
            return Incoming::Ping {
                sender: sender.to_string(),
                rest: fields[2..].join(":"),
            };
        }
        return Incoming::Other;
    }

    if head.starts_with("#TM") {
        return Incoming::TextMessage(packet.to_string());
    }

    if head.starts_with("#DL") {
        return Incoming::Heartbeat;
    }

    Incoming::Other
}

/// `$PO` —— 回一次 ping。
pub fn pong(callsign: &str, sender: &str, rest: &str) -> String {
    format!("$PO{callsign}:{sender}:{rest}")
}

/// 从一份 METAR 报文里取电台代号，用来把回包配给等它的那个请求。
pub fn metar_station(report: &str) -> Option<String> {
    report.split_whitespace().next().map(|s| s.to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 频率按协议压成 5 位：开头的 1 和小数点是隐含的。
    #[test]
    fn a_frequency_loses_its_leading_one_and_its_point() {
        assert_eq!(encode_frequency("118.000").as_deref(), Some("18000"));
        assert_eq!(encode_frequency("127.850").as_deref(), Some("27850"));
        assert_eq!(encode_frequency(" 121.8 ").as_deref(), Some("21800"));
        // 不是"去掉第一个字符"，是先补零到六位再去掉第一位。
        assert_eq!(encode_frequency("99.000").as_deref(), Some("99000"));
    }

    /// 认不出的频率给 `None`，**不是 0 也不是一个猜出来的值**。
    ///
    /// 发一个错的频率出去比不发更糟：席位会挂在一个没人听的频率上，
    /// 而在线列表里它看着一切正常。
    #[test]
    fn an_unreadable_frequency_is_refused_rather_than_guessed() {
        assert_eq!(encode_frequency(""), None);
        assert_eq!(encode_frequency("一二一点八"), None);
        assert_eq!(encode_frequency("99999"), None);
    }

    /// 包是冒号分隔的，正文里的冒号会破坏分帧。
    #[test]
    fn colons_and_newlines_cannot_survive_in_a_field() {
        assert_eq!(sanitize_line("a:b\r\nc"), "a b  c");
        assert_eq!(sanitize_line("  padded  "), "padded");
    }

    /// **密码也要过一遍。**
    ///
    /// 带冒号的密码让 `#AA` 后面的字段全部错位（服务器拒收），而 `redact` 按
    /// 固定下标打码——错位之后冒号后那一截密码会原样进日志。这条测试钉的就是
    /// 那个泄漏。
    #[test]
    fn a_colon_in_the_password_cannot_shift_the_fields_or_leak_into_the_log() {
        let id = Identity::new("ZSPD_ATIS", "1234", "pa:ss:word", "ATIS", 1);
        let packet = id.login_packet();
        assert_eq!(packet.split(':').count(), 7, "{packet}");
        assert!(packet.contains("pa ss word"), "{packet}");

        let logged = redact(&packet);
        assert!(!logged.contains("ss"), "the password leaked: {logged}");
        assert!(!logged.contains("word"), "the password leaked: {logged}");
        assert!(logged.contains(":***:"), "{logged}");
    }

    /// 呼号规则来自 can-fsd 的 `IsValidCallsign` / `IsATISCallsign`。
    ///
    /// 长度上限尤其容易踩：`ZSPD_D_ATIS` 有 11 个字符，**上限曾经是 10**，
    /// 正好卡死 vATIS 的分离通播。
    #[test]
    fn the_split_atis_callsigns_fit_inside_the_limit() {
        assert_eq!(check_atis_callsign("zspd_atis").as_deref(), Ok("ZSPD_ATIS"));
        assert_eq!(
            check_atis_callsign("ZSPD_D_ATIS").as_deref(),
            Ok("ZSPD_D_ATIS")
        );
        assert_eq!(
            check_atis_callsign("ZSPD_A_ATIS").as_deref(),
            Ok("ZSPD_A_ATIS")
        );
        // 上限正好卡在这里：12 个字符收，13 个不收。
        assert!(check_atis_callsign("ZSPDX_D_ATIS").is_ok());
        assert!(matches!(
            check_atis_callsign("ZSPDXY_D_ATIS"),
            Err(CallsignProblem::Length { len: 13, .. })
        ));
    }

    #[test]
    fn a_callsign_the_server_would_refuse_is_refused_here_first() {
        assert!(matches!(
            check_atis_callsign("A"),
            Err(CallsignProblem::Length { .. })
        ));
        assert!(matches!(
            check_atis_callsign("ZSPD_TOO_LONG_ATIS"),
            Err(CallsignProblem::Length { .. })
        ));
        assert!(matches!(
            check_atis_callsign("ZS!D_ATIS"),
            Err(CallsignProblem::Charset(_))
        ));
        assert!(matches!(
            check_atis_callsign("ZSPD_TWR"),
            Err(CallsignProblem::NotAtis(_))
        ));
    }

    #[test]
    fn atis_text_wraps_at_the_width_and_never_splits_a_word() {
        let lines = wrap_atis_text_to("one two three four five six", 9);
        assert_eq!(lines, vec!["one two", "three", "four five", "six"]);
        for line in &lines {
            assert!(line.chars().count() <= 9, "{line}");
        }
    }

    /// can-fsd 每个席位最多收 64 行，多出来的要**在这一侧丢掉**。
    #[test]
    fn no_more_than_sixty_four_lines_go_out() {
        let long = "word ".repeat(2000);
        assert_eq!(wrap_atis_text_to(&long, 10).len(), MAX_ATIS_LINES);
    }

    /// `$ID` 的第 9 格（challenge）**故意留空**：填了服务端就会发起 VATSIM
    /// 客户端质询，那套算法只有官方客户端有密钥表。
    #[test]
    fn the_id_packet_leaves_the_challenge_field_empty() {
        let id = Identity::new("ZSPD_ATIS", "1234", "pw", "ATIS", 1);
        let p = id.id_packet();
        assert!(p.starts_with("$IDZSPD_ATIS:SERVER:0001:"), "{p}");
        assert_eq!(
            p.split(':').count(),
            8,
            "a ninth field would be a challenge: {p}"
        );
    }

    /// 位置包的纬经度固定五位小数——服务端按字符串解析，位数变了会让席位
    /// 在雷达上跳一格。
    #[test]
    fn the_position_packet_has_the_shape_the_server_parses() {
        let pos = Position {
            frequency: "127.850".into(),
            facility: FACILITY_ATIS,
            vis_range: 50,
            rating: RATING_OBSERVER,
            latitude: 31.142_33,
            longitude: 121.790_84,
        };
        assert_eq!(
            pos.packet("ZSPD_ATIS").as_deref(),
            Some("%ZSPD_ATIS:27850:7:50:1:31.14233:121.79084:0")
        );
    }

    #[test]
    fn a_bad_frequency_makes_the_position_packet_refuse_to_exist() {
        let pos = Position {
            frequency: "nope".into(),
            facility: FACILITY_ATIS,
            vis_range: 50,
            rating: 1,
            latitude: 0.0,
            longitude: 0.0,
        };
        assert_eq!(pos.packet("ZSPD_ATIS"), None);
    }

    /// 飞行员客户端问的是 `ATIS`；**服务端自己的轮询收的是 `TEXTATIS`**
    /// （can-fsd `handler.go` 的 `handleTextATISResponse`），所以回给
    /// `SERVER` 时两种都发一遍，多余的那份会被忽略。
    #[test]
    fn the_server_gets_both_atis_and_textatis_but_a_pilot_gets_one() {
        let lines = vec!["LINE ONE".to_string(), "LINE TWO".to_string()];

        let to_pilot = atis_reply("ZSPD_ATIS", "CES123", &lines);
        assert_eq!(to_pilot.len(), 3);
        assert_eq!(to_pilot[0], "$CRZSPD_ATIS:CES123:ATIS:T:LINE ONE");
        assert_eq!(to_pilot[2], "$CRZSPD_ATIS:CES123:ATIS:E:2");
        assert!(!to_pilot.iter().any(|p| p.contains("TEXTATIS")));

        let to_server = atis_reply("ZSPD_ATIS", "SERVER", &lines);
        assert_eq!(to_server.len(), 6);
        assert!(to_server.iter().any(|p| p.contains(":TEXTATIS:T:LINE TWO")));
        assert_eq!(to_server[5], "$CRZSPD_ATIS:SERVER:TEXTATIS:E:2");
    }

    /// 末行是 `E` 带**行数**，不是行内容。数不对的话对面会一直等下一行。
    #[test]
    fn the_last_line_carries_the_count() {
        let empty: Vec<String> = Vec::new();
        assert_eq!(
            atis_reply("ZSPD_ATIS", "CES123", &empty),
            vec!["$CRZSPD_ATIS:CES123:ATIS:E:0"]
        );
    }

    /// **登录没有专门的成功包**，`$CR…CAPS` 就是确认。
    #[test]
    fn the_caps_reply_is_what_confirms_a_login() {
        assert_eq!(
            parse("$CRSERVER:ZSPD_ATIS:CAPS:ATCINFO=1", "ZSPD_ATIS"),
            Incoming::CapsReply
        );
        // 发给别人的那份不是我们的确认。
        assert_eq!(
            parse("$CRSERVER:ZBAA_ATIS:CAPS:ATCINFO=1", "ZSPD_ATIS"),
            Incoming::Other
        );
    }

    #[test]
    fn a_query_for_someone_else_is_not_ours_to_answer() {
        assert_eq!(
            parse("$CQCES123:ZSPD_ATIS:ATIS", "ZSPD_ATIS"),
            Incoming::AtisQuery {
                sender: "CES123".into()
            }
        );
        assert_eq!(
            parse("$CQCES123:ZBAA_ATIS:ATIS", "ZSPD_ATIS"),
            Incoming::Other
        );
    }

    /// METAR 报文里可能带冒号（备注段），拼回去时不能只取一格。
    #[test]
    fn a_metar_report_keeps_everything_after_the_third_field() {
        assert_eq!(
            parse(
                "$ARserver:ZSPD_ATIS:METAR:ZSPD 251300Z 09004MPS Q1013",
                "ZSPD_ATIS"
            ),
            Incoming::Metar("ZSPD 251300Z 09004MPS Q1013".into())
        );
        assert_eq!(
            parse("$ARserver:ZSPD_ATIS:METAR:ZSPD RMK A:B", "ZSPD_ATIS"),
            Incoming::Metar("ZSPD RMK A:B".into())
        );
    }

    #[test]
    fn an_error_packet_carries_its_code_and_message() {
        assert_eq!(
            parse(
                "$ERSERVER:ZSPD_ATIS:006:ZSPD_ATIS:invalid logon",
                "ZSPD_ATIS"
            ),
            Incoming::Error {
                code: "006".into(),
                message: "invalid logon".into()
            }
        );
    }

    #[test]
    fn a_ping_is_echoed_back_with_its_payload() {
        let got = parse("$PISERVER:ZSPD_ATIS:12345", "ZSPD_ATIS");
        assert_eq!(
            got,
            Incoming::Ping {
                sender: "SERVER".into(),
                rest: "12345".into()
            }
        );
        assert_eq!(
            pong("ZSPD_ATIS", "SERVER", "12345"),
            "$POZSPD_ATIS:SERVER:12345"
        );
    }

    #[test]
    fn a_heartbeat_and_a_text_message_are_told_apart() {
        assert_eq!(parse("#DLSERVER", "ZSPD_ATIS"), Incoming::Heartbeat);
        assert!(matches!(
            parse("#TMSERVER:ZSPD_ATIS:welcome", "ZSPD_ATIS"),
            Incoming::TextMessage(_)
        ));
    }

    #[test]
    fn the_metar_station_comes_off_the_front_of_the_report() {
        assert_eq!(metar_station("zspd 251300Z").as_deref(), Some("ZSPD"));
        assert_eq!(metar_station("   "), None);
    }

    /// `redact` 只动 `#AA`，别的包原样进日志。
    #[test]
    fn only_the_login_packet_is_redacted() {
        assert_eq!(redact("%ZSPD_ATIS:27850:7"), "%ZSPD_ATIS:27850:7");
    }
}
