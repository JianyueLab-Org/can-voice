//! 配置模型：`Profile` → `Station` → `Preset`，层级照搬 vATIS。
//!
//! ```text
//! Profile   一套配置，含若干席位
//! Station   一个机场的通播席位：ICAO、频率、类型、情报字母范围、若干预设
//! Preset    一份模板（含机场条件、NOTAM 自由文本），随天气/跑道构型切换
//! ```
//!
//! 情报字母在 METAR 变化时前进一格，可以限定取值范围——离场和进场分别用不同
//! 字母段是 vATIS 的 Code Range，避免飞行员把两份通播搞混。
//!
//! 只有数据和规则，没有别的，所以这一层可以直接测。
//!
//! # 和 `can-audio/atis/profile.py` 有意不同的三处
//!
//! **一、没有 `channel`。** 那边的席位有一个 `FREQ_<六位千赫>` 频道名，因为
//! Mumble 用频道名路由。can-voice 不是：频率在协议里就是一个 `u32`，服务端
//! 把它当**不透明路由键**（`server/internal/wire/header.go` 写着这一条）。
//! 把频道名搬过来等于把一个已经被设计掉的概念请回来。
//!
//! **二、没有"Profile 自己读写文件"那条老路。** 那边的 `Profile(path=…)` 只为
//! 兼容老测试留着，注释自己说存盘归 `ProfileSet` 管。这里只有一条路。
//!
//! **三、错误不是界面文字。** 那边直接抛出 `t("station.duplicate", …)`，本地化
//! 串一路穿到模型层。这里抛 [`ProfileError`]，文案由前端决定——同一个错误在
//! 中英两种界面下要说两种话，而模型不该知道现在是哪一种。

use crate::airports;
use crate::default_names::{DEFAULT_PRESET_NAME, DEFAULT_PROFILE_NAME};
use crate::template::{Contractions, DEFAULT_TEMPLATE};
use can_voice_i18n::Message;
use serde::{Deserialize, Serialize};

const LETTERS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// 通播类型决定网络上的呼号后缀，和 vATIS 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AtisType {
    #[default]
    Combined,
    Departure,
    Arrival,
}

impl AtisType {
    pub fn suffix(self) -> &'static str {
        match self {
            AtisType::Combined => "_ATIS",
            AtisType::Departure => "_D_ATIS",
            AtisType::Arrival => "_A_ATIS",
        }
    }
}

/// 通播稿的语言，**不是界面语言**：这是播给飞行员听的，一个英文界面的操作者
/// 照样可能在管一份中文通播。
///
/// 中文稿由 [`crate::chinese`] 单独渲染——中文通播不是英文的逐词翻译，
/// 语序和数字读法都是民航自己的一套。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceLanguage {
    #[default]
    En,
    Zh,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("a station with callsign {0} already exists")]
    DuplicateStation(String),
    #[error("the profile name is empty")]
    EmptyName,
    #[error("a profile named {0} already exists")]
    DuplicateProfile(String),
    #[error("there is no profile named {0}")]
    MissingProfile(String),
    #[error("the last profile cannot be removed")]
    LastProfile,
}

impl ProfileError {
    /// 给人看的那一句（#29）。`Display` 是给日志的英文。
    pub fn message(&self) -> Message {
        match self {
            ProfileError::DuplicateStation(callsign) => {
                Message::new("problem.profile.duplicate_station").with("callsign", callsign)
            }
            ProfileError::EmptyName => Message::new("problem.profile.empty_name"),
            ProfileError::DuplicateProfile(name) => {
                Message::new("problem.profile.duplicate_profile").with("name", name)
            }
            ProfileError::MissingProfile(name) => {
                Message::new("problem.profile.missing_profile").with("name", name)
            }
            ProfileError::LastProfile => Message::new("problem.profile.last_profile"),
        }
    }
}

fn default_template() -> String {
    DEFAULT_TEMPLATE.to_string()
}

/// 为什么是中文、为什么不翻译，见 [`crate::default_names`]。
fn default_preset_name() -> String {
    DEFAULT_PRESET_NAME.to_string()
}

/// 一份模板，随天气或跑道构型切换。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    #[serde(default = "default_preset_name")]
    pub name: String,
    #[serde(default = "default_template")]
    pub template: String,
    #[serde(default)]
    pub airport_conditions: String,
    #[serde(default)]
    pub notams: String,
    #[serde(default)]
    pub transition_level: String,
    /// 中文稿念的跑道。**跟着预设走而不是跟着席位**——切到"北向"时英文稿的
    /// `ARR RWY` 会变，中文稿要是还念着南向的跑道，同一份通播里两种语言互相
    /// 矛盾，而大陆机场是双语播的，两边都有人听。留空则回退到席位上的
    /// `chinese_runway`。
    #[serde(default)]
    pub chinese_runway: String,
    /// 收尾语跟着预设走。不同构型要交代的事不一样，比如「并确认能否执行 RNAV
    /// 程序」只该出现在 RNAV 离场可用的那份稿子里。留空用内置那句。
    #[serde(default)]
    pub closing: String,
    /// 中文稿的附加文本。中文通播不是英文的逐词翻译（[`crate::chinese`] 是从
    /// METAR 独立渲染的），跑道构型、放行频率、应答机模式这些注意事项在中文侧
    /// 没有对应字段，整段写在这里，接在气象之后念。
    #[serde(default)]
    pub chinese_extra: String,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            name: default_preset_name(),
            template: default_template(),
            airport_conditions: String::new(),
            notams: String::new(),
            transition_level: String::new(),
            chinese_runway: String::new(),
            closing: String::new(),
            chinese_extra: String::new(),
        }
    }
}

fn default_frequency() -> String {
    "118.000".to_string()
}

fn default_code_range() -> (char, char) {
    ('A', 'Z')
}

/// 一个机场的通播席位。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Station {
    pub identifier: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_frequency")]
    pub frequency: String,
    #[serde(default)]
    pub atis_type: AtisType,
    /// 情报字母的可用范围，闭区间。
    #[serde(default = "default_code_range")]
    pub code_range: (char, char),
    #[serde(default)]
    pub letter: char,
    #[serde(default)]
    pub presets: Vec<Preset>,
    /// 缩略语：模板里写 `@变量名`，渲染时替换。文字和语音两种形态，
    /// 和气象要素一样（vATIS 的 Contractions）。
    #[serde(default)]
    pub contractions: Contractions,
    /// 席位在 FSD 上报的位置。没填就按 ICAO 查机场坐标——留成 0/0 的话
    /// **席位会显示在几内亚湾外海**。
    #[serde(default)]
    pub latitude: f64,
    #[serde(default)]
    pub longitude: f64,
    #[serde(default)]
    pub voice_language: VoiceLanguage,
    /// 中文稿里念的机场名和跑道，比如"上海浦东"和"三六左"。留空就用识别码。
    #[serde(default)]
    pub chinese_name: String,
    #[serde(default)]
    pub chinese_runway: String,
}

impl Station {
    pub fn new(identifier: &str) -> Self {
        let mut s = Self {
            identifier: identifier.trim().to_uppercase(),
            name: String::new(),
            frequency: default_frequency(),
            atis_type: AtisType::default(),
            code_range: default_code_range(),
            letter: 'A',
            presets: vec![Preset::default()],
            contractions: Contractions::new(),
            latitude: 0.0,
            longitude: 0.0,
            voice_language: VoiceLanguage::default(),
            chinese_name: String::new(),
            chinese_runway: String::new(),
        };
        s.normalise();
        s
    }

    /// 读进来之后要跑一遍：识别码大写、字母落在范围里、坐标补上。
    ///
    /// serde 只保证字段在，不保证它们讲得通——**一份手改过的 json 照样要能用**。
    pub fn normalise(&mut self) {
        self.identifier = self.identifier.trim().to_uppercase();
        if self.presets.is_empty() {
            self.presets.push(Preset::default());
        }
        if !LETTERS.contains(self.code_range.0) || !LETTERS.contains(self.code_range.1) {
            self.code_range = default_code_range();
        }
        if !self.set_letter(self.letter) {
            self.letter = self.code_range.0;
        }
        if self.latitude == 0.0 && self.longitude == 0.0 {
            if let Some((lat, lon)) = airports::coordinates(&self.identifier) {
                self.latitude = lat;
                self.longitude = lon;
            }
        }
    }

    /// 网络上的呼号：`ZSPD` + `_ATIS` / `_D_ATIS` / `_A_ATIS`。
    pub fn callsign(&self) -> String {
        format!("{}{}", self.identifier, self.atis_type.suffix())
    }

    /// 频率，单位千赫。协议里频率就是这个 `u32`。
    ///
    /// 认不出的频率给 `None` 而不是 0：0 是一个**合法的路由键**
    /// （服务端不做范围校验），错当频率发出去会让人在一个谁也不在的频率上说话。
    pub fn frequency_khz(&self) -> Option<u32> {
        let mhz: f64 = self.frequency.trim().parse().ok()?;
        let khz = (mhz * 1000.0).round();
        if (0.0..=u32::MAX as f64).contains(&khz) {
            Some(khz as u32)
        } else {
            None
        }
    }

    pub fn label(&self) -> String {
        format!("{}  {}", self.callsign(), self.frequency)
    }

    pub fn preset(&self, name: &str) -> Option<&Preset> {
        self.presets
            .iter()
            .find(|p| p.name == name)
            .or_else(|| self.presets.first())
    }

    /// 范围内的情报字母，**允许跨 Z 回绕**（例如 X..C）。
    pub fn letters_in_range(&self) -> Vec<char> {
        let all: Vec<char> = LETTERS.chars().collect();
        let start = all.iter().position(|c| *c == self.code_range.0);
        let end = all.iter().position(|c| *c == self.code_range.1);
        let (Some(start), Some(end)) = (start, end) else {
            return all;
        };
        if start <= end {
            all[start..=end].to_vec()
        } else {
            all[start..].iter().chain(&all[..=end]).copied().collect()
        }
    }

    /// 推进一格情报字母，到范围末尾就绕回开头。
    pub fn advance_letter(&mut self) -> char {
        let available = self.letters_in_range();
        self.letter = match available.iter().position(|c| *c == self.letter) {
            Some(i) => available[(i + 1) % available.len()],
            None => available[0],
        };
        self.letter
    }

    /// 设一个字母。不在范围里就**拒绝**并保持原样，返回 `false`。
    pub fn set_letter(&mut self, letter: char) -> bool {
        let letter = letter.to_ascii_uppercase();
        if self.letters_in_range().contains(&letter) {
            self.letter = letter;
            true
        } else {
            false
        }
    }
}

/// 配置文件名。
pub const DEFAULT_PROFILE_PATH: &str = "atis_profile.json";

/// 为什么是中文、为什么不翻译，见 [`crate::default_names`]。
fn default_profile_name() -> String {
    DEFAULT_PROFILE_NAME.to_string()
}

/// 一套席位配置。vATIS 的说法：一份 profile 装一组席位。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default = "default_profile_name")]
    pub name: String,
    #[serde(default)]
    pub stations: Vec<Station>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: default_profile_name(),
            stations: Vec::new(),
        }
    }
}

impl Profile {
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            stations: Vec::new(),
        }
    }

    pub fn add(&mut self, station: Station) -> Result<(), ProfileError> {
        let callsign = station.callsign();
        if self.get(&callsign).is_some() {
            return Err(ProfileError::DuplicateStation(callsign));
        }
        self.stations.push(station);
        self.stations.sort_by_key(|s| s.callsign());
        Ok(())
    }

    pub fn remove(&mut self, callsign: &str) -> bool {
        let before = self.stations.len();
        self.stations.retain(|s| s.callsign() != callsign);
        self.stations.len() != before
    }

    pub fn get(&self, callsign: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.callsign() == callsign)
    }

    pub fn get_mut(&mut self, callsign: &str) -> Option<&mut Station> {
        self.stations.iter_mut().find(|s| s.callsign() == callsign)
    }

    fn normalise(&mut self) {
        if self.name.trim().is_empty() {
            self.name = default_profile_name();
        }
        for s in &mut self.stations {
            s.normalise();
        }
        self.stations.sort_by_key(|s| s.callsign());
    }
}

/// 整个文件：几份 profile，加上当前用哪一份。
///
/// **为什么要多份**：同一个人可能同时管华东和华北，两边的席位、模板、跑道构型
/// 完全不同；混在一张列表里，值班时要在十几个不相关的席位里找自己那两个。
/// vATIS 也是这个模型。
#[derive(Debug, Clone)]
pub struct ProfileSet {
    pub path: std::path::PathBuf,
    profiles: Vec<Profile>,
    active_name: String,
}

/// 文件在磁盘上的形状。
#[derive(Debug, Deserialize)]
struct FileShape {
    #[serde(default)]
    active: Option<String>,
    /// 新格式。
    #[serde(default)]
    profiles: Option<Vec<Profile>>,
    /// **老格式：整个文件就是一份配置。**
    ///
    /// 现有的 `atis_profile.json` 是 `{"stations": [...]}`，没有 profile 这一层。
    /// 读到那种形状就当成一份名叫"默认"的 profile——不能因为加了多配置这个
    /// 功能，让所有人打开就是空配置。
    #[serde(default)]
    stations: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
struct SaveShape<'a> {
    active: &'a str,
    profiles: &'a [Profile],
}

impl ProfileSet {
    /// 从文件读一份。读不出来就是一份空的——**不报错**：配置文件坏了不该让
    /// 通播客户端开不起来，而一份空配置界面上一眼就看得出。
    pub fn load(path: impl Into<std::path::PathBuf>) -> Self {
        let path = path.into();
        let mut set = Self {
            path,
            profiles: Vec::new(),
            active_name: default_profile_name(),
        };
        match std::fs::read_to_string(&set.path) {
            Ok(raw) => set.read_str(&raw),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(error = %e, "could not read the profile file"),
        }
        set.ensure_one();
        set
    }

    fn read_str(&mut self, raw: &str) {
        let parsed: FileShape = match serde_json::from_str(raw) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "the profile file is not valid json");
                return;
            }
        };
        if let Some(profiles) = parsed.profiles {
            self.profiles = profiles;
            self.active_name = parsed.active.unwrap_or_else(default_profile_name);
        } else if let Some(stations) = parsed.stations {
            tracing::info!(
                "read a profile file in the old shape, treating it as one profile named {DEFAULT_PROFILE_NAME:?}"
            );
            self.profiles.push(Profile {
                name: default_profile_name(),
                stations: stations_from(stations),
            });
            self.active_name = default_profile_name();
        }
        for p in &mut self.profiles {
            p.normalise();
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let payload = SaveShape {
            active: &self.active_name,
            profiles: &self.profiles,
        };
        let json = serde_json::to_string_pretty(&payload)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&self.path, json)
    }

    pub fn names(&self) -> Vec<String> {
        self.profiles.iter().map(|p| p.name.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// 文件是空的也保证有一份，界面不用到处判 `None`。
    fn ensure_one(&mut self) {
        if self.profiles.is_empty() {
            self.profiles.push(Profile::default());
        }
        if !self.profiles.iter().any(|p| p.name == self.active_name) {
            self.active_name = self.profiles[0].name.clone();
        }
    }

    pub fn active_name(&self) -> &str {
        &self.active_name
    }

    pub fn active(&mut self) -> &mut Profile {
        self.ensure_one();
        let name = self.active_name.clone();
        self.profiles
            .iter_mut()
            .find(|p| p.name == name)
            .expect("ensure_one guarantees the active profile exists")
    }

    /// 新建一份空的。名字重复时报错。
    ///
    /// Python 那边这里踩过一个坑值得记着：`Profile` 定义了 `__len__`，所以
    /// **一份还没有席位的配置是假值**，`if self.get(name):` 会认为它不存在，
    /// 于是允许重名建第二份。Rust 的 `Option` 让这个坑不存在，但
    /// `a_duplicate_name_is_refused_even_when_the_existing_one_is_empty`
    /// 那条测试仍然留着——它钉的是行为，不是当年那个写法。
    pub fn add(&mut self, name: &str) -> Result<&Profile, ProfileError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProfileError::EmptyName);
        }
        if self.get(name).is_some() {
            return Err(ProfileError::DuplicateProfile(name.to_string()));
        }
        self.profiles.push(Profile::named(name));
        Ok(self.profiles.last().expect("just pushed"))
    }

    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), ProfileError> {
        let new = new.trim();
        if new.is_empty() {
            return Err(ProfileError::EmptyName);
        }
        if self.get(old).is_none() {
            return Err(ProfileError::MissingProfile(old.to_string()));
        }
        if new != old && self.get(new).is_some() {
            return Err(ProfileError::DuplicateProfile(new.to_string()));
        }
        if let Some(p) = self.profiles.iter_mut().find(|p| p.name == old) {
            p.name = new.to_string();
        }
        if self.active_name == old {
            self.active_name = new.to_string();
        }
        Ok(())
    }

    /// 删一份。**最后一份不许删**——删光了界面就没有可操作的对象了。
    pub fn remove(&mut self, name: &str) -> Result<bool, ProfileError> {
        if self.profiles.len() <= 1 {
            return Err(ProfileError::LastProfile);
        }
        let before = self.profiles.len();
        self.profiles.retain(|p| p.name != name);
        if self.profiles.len() == before {
            return Ok(false);
        }
        if self.active_name == name {
            self.active_name = self.profiles[0].name.clone();
        }
        Ok(true)
    }

    pub fn select(&mut self, name: &str) -> bool {
        if self.get(name).is_none() {
            return false;
        }
        self.active_name = name.to_string();
        true
    }
}

/// 一份 profile 里的席位列表。**认不出的单个跳过，不连累整份。**
///
/// 一个手改坏了的席位不该让另外十一个一起消失。
fn stations_from(entries: Vec<serde_json::Value>) -> Vec<Station> {
    let mut stations = Vec::new();
    for entry in entries {
        match serde_json::from_value::<Station>(entry) {
            Ok(mut s) => {
                s.normalise();
                stations.push(s);
            }
            Err(e) => tracing::warn!(error = %e, "skipping an unrecognisable station"),
        }
    }
    stations.sort_by_key(|s| s.callsign());
    stations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(icao: &str) -> Station {
        Station::new(icao)
    }

    #[test]
    fn callsign_by_type() {
        assert_eq!(station("zspd").callsign(), "ZSPD_ATIS");
        let mut s = station("ZSPD");
        s.atis_type = AtisType::Departure;
        assert_eq!(s.callsign(), "ZSPD_D_ATIS");
        s.atis_type = AtisType::Arrival;
        assert_eq!(s.callsign(), "ZSPD_A_ATIS");
    }

    /// 频率就是一个千赫整数。
    ///
    /// **没有 `FREQ_127850` 这样的频道名**，这是和 can-audio 最大的一处不同：
    /// 那边靠 Mumble 的频道名路由，can-voice 的协议里频率是一个不透明的 u32。
    #[test]
    fn the_frequency_is_a_number_of_kilohertz_and_nothing_else() {
        let mut s = station("ZSPD");
        s.frequency = "127.850".into();
        assert_eq!(s.frequency_khz(), Some(127_850));
        s.frequency = "118".into();
        assert_eq!(s.frequency_khz(), Some(118_000));
        // 认不出的给 None，不是 0 —— 0 是一个合法的路由键，
        // 错当频率发出去会让人在一个谁也不在的频率上说话。
        s.frequency = "".into();
        assert_eq!(s.frequency_khz(), None);
        s.frequency = "一二一点八".into();
        assert_eq!(s.frequency_khz(), None);
    }

    #[test]
    fn letter_advances_and_wraps() {
        let mut s = station("ZSPD");
        assert_eq!(s.letter, 'A');
        assert_eq!(s.advance_letter(), 'B');
        assert!(s.set_letter('Z'));
        assert_eq!(s.advance_letter(), 'A');
    }

    #[test]
    fn code_range_limits_the_letters() {
        let mut s = station("ZSPD");
        s.code_range = ('A', 'C');
        assert_eq!(s.letters_in_range(), vec!['A', 'B', 'C']);
        s.letter = 'C';
        assert_eq!(s.advance_letter(), 'A');
        assert!(!s.set_letter('M'), "范围外的字母不该被接受");
        assert_eq!(s.letter, 'A', "被拒绝之后原来的字母要保持不变");
    }

    /// 离场和进场用不同字母段是 vATIS 的 Code Range，避免飞行员把两份通播
    /// 搞混——**所以范围必须允许跨 Z 回绕**，否则 Y..B 这种段根本写不出来。
    #[test]
    fn code_range_can_wrap_past_z() {
        let mut s = station("ZSPD");
        s.code_range = ('Y', 'B');
        assert_eq!(s.letters_in_range(), vec!['Y', 'Z', 'A', 'B']);
        s.letter = 'Z';
        assert_eq!(s.advance_letter(), 'A');
    }

    /// 席位不填坐标就会落在 0/0 ——**几内亚湾外海**，而那看起来像一个真实的
    /// 席位，没有任何东西会报错。
    #[test]
    fn a_station_fills_its_position_from_the_icao() {
        let s = station("RJAA");
        assert!((s.latitude - 35.766_94).abs() < 1e-4, "{}", s.latitude);
        assert!((s.longitude - 140.387_78).abs() < 1e-4, "{}", s.longitude);
    }

    /// 手填的坐标优先——机场基准点未必是塔台位置。
    #[test]
    fn a_manual_position_wins() {
        let mut s = station("RJAA");
        s.latitude = 35.5;
        s.longitude = 140.1;
        s.normalise();
        assert_eq!(s.latitude, 35.5);
    }

    #[test]
    fn an_unknown_airport_leaves_zero() {
        let s = station("XXXX");
        assert_eq!((s.latitude, s.longitude), (0.0, 0.0));
    }

    #[test]
    fn add_and_reject_duplicates() {
        let mut p = Profile::default();
        p.add(station("ZSPD")).expect("first");
        assert_eq!(
            p.add(station("ZSPD")),
            Err(ProfileError::DuplicateStation("ZSPD_ATIS".into()))
        );
        // 类型不同就是不同席位，可以共存。
        let mut dep = station("ZSPD");
        dep.atis_type = AtisType::Departure;
        p.add(dep).expect("a departure station is a different one");
        assert_eq!(p.stations.len(), 2);
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("can-voice-atis-{name}-{}.json", std::process::id()));
        p
    }

    #[test]
    fn a_profile_survives_a_round_trip() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);

        let mut set = ProfileSet::load(&path);
        let mut s = station("ZSPD");
        s.name = "浦东".into();
        s.frequency = "127.850".into();
        s.code_range = ('A', 'M');
        s.presets = vec![Preset {
            name: "白天".into(),
            template: "[FACILITY] [ATIS_LETTER]".into(),
            airport_conditions: "跑道 35L".into(),
            ..Default::default()
        }];
        s.advance_letter();
        set.active().add(s).expect("add");
        set.save().expect("save");

        let mut restored = ProfileSet::load(&path);
        let profile = restored.active();
        assert_eq!(profile.stations.len(), 1);
        let loaded = profile.get("ZSPD_ATIS").expect("ZSPD_ATIS");
        assert_eq!(loaded.name, "浦东");
        assert_eq!(loaded.frequency, "127.850");
        assert_eq!(loaded.letter, 'B');
        assert_eq!(loaded.code_range, ('A', 'M'));
        assert_eq!(loaded.presets[0].airport_conditions, "跑道 35L");
        // 坐标也要活过一个来回，否则每次重启席位都回到几内亚湾。
        assert!(
            (loaded.latitude - 31.142_33).abs() < 1e-4,
            "{}",
            loaded.latitude
        );

        let _ = std::fs::remove_file(&path);
    }

    /// **一个手改坏了的席位不该让另外十一个一起消失。**
    #[test]
    fn a_bad_entry_is_skipped_rather_than_losing_the_rest() {
        let path = temp_path("badentry");
        std::fs::write(
            &path,
            r#"{"stations": [{"identifier": "ZSPD"}, {"nope": 1}, {"identifier": "ZBAA"}]}"#,
        )
        .expect("write");
        let mut set = ProfileSet::load(&path);
        let names: Vec<String> = set.active().stations.iter().map(|s| s.callsign()).collect();
        assert_eq!(names, vec!["ZBAA_ATIS", "ZSPD_ATIS"]);
        let _ = std::fs::remove_file(&path);
    }

    /// **老文件必须读得进来。**
    ///
    /// 现有的 `atis_profile.json` 是 `{"stations": [...]}`，没有 profile 这一层。
    /// 不能因为加了多配置这个功能，让所有人打开就是空配置。
    #[test]
    fn a_file_in_the_old_shape_becomes_one_profile() {
        let path = temp_path("oldshape");
        std::fs::write(&path, r#"{"stations": [{"identifier": "ZSPD"}]}"#).expect("write");
        let mut set = ProfileSet::load(&path);
        assert_eq!(set.len(), 1);
        assert_eq!(set.names(), vec!["默认"]);
        assert!(set.active().get("ZSPD_ATIS").is_some());
        let _ = std::fs::remove_file(&path);
    }

    /// 老预设里没有 `chinese_runway` / `closing` / `chinese_extra`，
    /// 读出来该是空串而不是炸掉——**升级不该改变已有席位的行为**。
    #[test]
    fn a_preset_without_the_newer_fields_still_loads() {
        let p: Preset = serde_json::from_str(r#"{"name": "默认"}"#).expect("parse");
        assert_eq!(p.chinese_runway, "");
        assert_eq!(p.closing, "");
        assert_eq!(p.chinese_extra, "");
        // 没写模板就用内置那份，不是空模板。
        assert_eq!(p.template, DEFAULT_TEMPLATE);
    }

    /// 老席位里没有 `voice_language` / `chinese_name`，缺省按英文。
    #[test]
    fn a_station_without_the_newer_fields_still_loads() {
        let mut s: Station = serde_json::from_str(r#"{"identifier": "ZSPD"}"#).expect("parse");
        s.normalise();
        assert_eq!(s.voice_language, VoiceLanguage::En);
        assert_eq!(s.chinese_name, "");
        assert_eq!(s.frequency, "118.000");
        assert_eq!(s.presets.len(), 1, "空的预设列表要补上一份默认的");
    }

    /// 重名要被拒——**哪怕现有那一份还是空的**。
    ///
    /// Python 那边这里踩过一个坑：`Profile` 定义了 `__len__`，所以一份没有席位
    /// 的配置是假值，`if self.get(name):` 认为它不存在，于是允许建第二份同名的，
    /// 之后选中哪一份全看顺序。这条钉的是行为，Rust 这边坑不存在，但行为要一样。
    #[test]
    fn a_duplicate_name_is_refused_even_when_the_existing_one_is_empty() {
        let mut set = ProfileSet::load(temp_path("dup-never-written"));
        set.add("华东").expect("first");
        assert!(set.get("华东").expect("empty profile").stations.is_empty());
        assert_eq!(
            set.add("华东").err(),
            Some(ProfileError::DuplicateProfile("华东".into()))
        );
        assert_eq!(set.add("  ").err(), Some(ProfileError::EmptyName));
    }

    /// **最后一份不许删**——删光了界面就没有可操作的对象了。
    #[test]
    fn the_last_profile_cannot_be_removed() {
        let mut set = ProfileSet::load(temp_path("lastone"));
        assert_eq!(set.len(), 1);
        assert_eq!(set.remove("默认").err(), Some(ProfileError::LastProfile));
        set.add("华北").expect("add");
        assert_eq!(set.remove("默认"), Ok(true));
        assert_eq!(set.active_name(), "华北", "删掉当前那份要改选还在的那份");
    }

    #[test]
    fn renaming_follows_the_active_selection() {
        let mut set = ProfileSet::load(temp_path("rename"));
        set.add("华东").expect("add");
        assert!(set.select("华东"));
        set.rename("华东", "华东区").expect("rename");
        assert_eq!(set.active_name(), "华东区");
        assert_eq!(
            set.rename("不存在", "x").err(),
            Some(ProfileError::MissingProfile("不存在".into()))
        );
        assert_eq!(
            set.rename("华东区", "默认").err(),
            Some(ProfileError::DuplicateProfile("默认".into()))
        );
        // 改成自己原来的名字不算重名。
        set.rename("华东区", "华东区")
            .expect("renaming to itself is fine");
    }

    /// 文件坏了不该让通播客户端开不起来——**读不出来就是一份空的**，
    /// 而一份空配置界面上一眼就看得出。
    #[test]
    fn a_corrupt_file_yields_an_empty_set_rather_than_an_error() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{not json at all").expect("write");
        let mut set = ProfileSet::load(&path);
        assert_eq!(set.len(), 1);
        assert!(set.active().stations.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    /// 预设按名字取，取不到退回第一份——**不能返回 None 让稿子变成空的**。
    #[test]
    fn an_unknown_preset_falls_back_to_the_first() {
        let s = station("ZSPD");
        assert_eq!(s.preset("不存在").map(|p| p.name.as_str()), Some("默认"));
    }
}
