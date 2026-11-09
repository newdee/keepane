//! Feishu (Lark) as a way in (docs/design/feishu.md): what does not need
//! the server. The long connection's frames (Feishu's `pbbp2` protobuf,
//! written by hand: two messages, eleven fields), its parts put back
//! together, the events read, the commands a chat sends, and the
//! requests to Feishu's open platform, as data. The server's side (who is
//! let in, which pane, what is sent back) is `server::feishu`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Feishu's open platform, in China.
pub const FEISHU: &str = "https://open.feishu.cn";
/// Lark's, everywhere else.
pub const LARK: &str = "https://open.larksuite.com";

/// Where the open platform is: `feishu`, `lark`, or an address of one's
/// own (`http://127.0.0.1:9000`, a test's), without a `/` at its end.
pub fn base_url(domain: &str) -> String {
    match domain.trim() {
        "" | "feishu" => FEISHU.into(),
        "lark" => LARK.into(),
        d => d.trim_end_matches('/').into(),
    }
}

/// Whether `feishu-domain` takes `value`.
pub fn valid_domain(value: &str) -> bool {
    let v = value.trim();
    matches!(v, "feishu" | "lark")
        || ((v.starts_with("http://") || v.starts_with("https://"))
            && !v.contains(|c: char| c.is_whitespace() || c == '"' || c == '\\'))
}

/// A frame's `method`: control (ping, pong) or data (an event).
pub const METHOD_CONTROL: i32 = 0;
pub const METHOD_DATA: i32 = 1;

/// One frame of the long connection (`pbbp2.Frame`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub seq_id: u64,
    pub log_id: u64,
    pub service: i32,
    pub method: i32,
    pub headers: Vec<(String, String)>,
    pub payload_encoding: String,
    pub payload_type: String,
    pub payload: Vec<u8>,
    pub log_id_new: String,
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_key(out: &mut Vec<u8>, field: u32, wire: u8) {
    put_varint(out, (u64::from(field) << 3) | u64::from(wire));
}

fn put_bytes(out: &mut Vec<u8>, field: u32, b: &[u8]) {
    put_key(out, field, 2);
    put_varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

/// Protobuf's varints, read; None past the end or past ten bytes.
fn get_varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *b.get(*at)?;
        *at += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some(v);
        }
    }
    None
}

/// A length-delimited field's bytes.
fn get_bytes<'a>(b: &'a [u8], at: &mut usize) -> Option<&'a [u8]> {
    let n = usize::try_from(get_varint(b, at)?).ok()?;
    let end = at.checked_add(n).filter(|e| *e <= b.len())?;
    let s = &b[*at..end];
    *at = end;
    Some(s)
}

/// Past a field this reader does not know.
fn skip(b: &[u8], at: &mut usize, wire: u8) -> Option<()> {
    match wire {
        0 => get_varint(b, at).map(|_| ()),
        1 => (*at + 8 <= b.len()).then(|| *at += 8),
        2 => get_bytes(b, at).map(|_| ()),
        5 => (*at + 4 <= b.len()).then(|| *at += 4),
        _ => None,
    }
}

fn text(b: &[u8]) -> Result<String, String> {
    String::from_utf8(b.to_vec()).map_err(|_| "a frame's text is not UTF-8".to_string())
}

impl Frame {
    /// A header's value.
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// A header set, in place of one by that name.
    pub fn set_header(&mut self, key: &str, value: &str) {
        self.headers.retain(|(k, _)| k != key);
        self.headers.push((key.into(), value.into()));
    }

    /// The ping keepane sends every `PingInterval` (`service`: the
    /// connection's, from its address).
    pub fn ping(service: i32) -> Frame {
        Frame { service, method: METHOD_CONTROL, headers: vec![("type".into(), "ping".into())], ..Frame::default() }
    }

    /// The bytes on the wire. The four required fields always; the others
    /// when they hold something.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.payload.len());
        put_key(&mut out, 1, 0);
        put_varint(&mut out, self.seq_id);
        put_key(&mut out, 2, 0);
        put_varint(&mut out, self.log_id);
        // int32: a negative one as ten bytes, sign extended.
        put_key(&mut out, 3, 0);
        put_varint(&mut out, i64::from(self.service) as u64);
        put_key(&mut out, 4, 0);
        put_varint(&mut out, i64::from(self.method) as u64);
        for (k, v) in &self.headers {
            let mut h = Vec::new();
            put_bytes(&mut h, 1, k.as_bytes());
            put_bytes(&mut h, 2, v.as_bytes());
            put_bytes(&mut out, 5, &h);
        }
        for (field, s) in [(6, &self.payload_encoding), (7, &self.payload_type)] {
            if !s.is_empty() {
                put_bytes(&mut out, field, s.as_bytes());
            }
        }
        if !self.payload.is_empty() {
            put_bytes(&mut out, 8, &self.payload);
        }
        if !self.log_id_new.is_empty() {
            put_bytes(&mut out, 9, self.log_id_new.as_bytes());
        }
        out
    }

    /// A frame from the wire; what is wrong with it otherwise.
    pub fn decode(b: &[u8]) -> Result<Frame, String> {
        let bad = || "not a Feishu frame".to_string();
        let mut f = Frame::default();
        let mut at = 0;
        while at < b.len() {
            let key = get_varint(b, &mut at).ok_or_else(bad)?;
            let (field, wire) = (key >> 3, (key & 7) as u8);
            match (field, wire) {
                (1, 0) => f.seq_id = get_varint(b, &mut at).ok_or_else(bad)?,
                (2, 0) => f.log_id = get_varint(b, &mut at).ok_or_else(bad)?,
                (3, 0) => f.service = get_varint(b, &mut at).ok_or_else(bad)? as i32,
                (4, 0) => f.method = get_varint(b, &mut at).ok_or_else(bad)? as i32,
                (5, 2) => {
                    let h = get_bytes(b, &mut at).ok_or_else(bad)?;
                    let (mut k, mut v) = (String::new(), String::new());
                    let mut i = 0;
                    while i < h.len() {
                        let key = get_varint(h, &mut i).ok_or_else(bad)?;
                        match (key >> 3, (key & 7) as u8) {
                            (1, 2) => k = text(get_bytes(h, &mut i).ok_or_else(bad)?)?,
                            (2, 2) => v = text(get_bytes(h, &mut i).ok_or_else(bad)?)?,
                            (_, w) => skip(h, &mut i, w).ok_or_else(bad)?,
                        }
                    }
                    f.headers.push((k, v));
                }
                (6, 2) => f.payload_encoding = text(get_bytes(b, &mut at).ok_or_else(bad)?)?,
                (7, 2) => f.payload_type = text(get_bytes(b, &mut at).ok_or_else(bad)?)?,
                (8, 2) => f.payload = get_bytes(b, &mut at).ok_or_else(bad)?.to_vec(),
                (9, 2) => f.log_id_new = text(get_bytes(b, &mut at).ok_or_else(bad)?)?,
                (_, w) => skip(b, &mut at, w).ok_or_else(bad)?,
            }
        }
        Ok(f)
    }

    /// What keepane writes back for a data frame: the same frame, its
    /// payload the answer Feishu waits for (within 3 seconds, or it sends
    /// the event again), `biz_rt` how long it took in milliseconds.
    pub fn ack(&self, millis: u128) -> Frame {
        let mut f = self.clone();
        f.payload = br#"{"code":200,"headers":null,"data":null}"#.to_vec();
        f.set_header("biz_rt", &millis.to_string());
        f
    }
}

/// An event comes in parts when it is big: kept by message until all are
/// here, at most this long.
const PARTS_KEPT: Duration = Duration::from_secs(10);

/// When the first part of an event came, and the parts by number.
type Open = (Instant, Vec<Option<Vec<u8>>>);

/// The parts of events not yet whole.
#[derive(Default)]
pub struct Parts {
    open: HashMap<String, Open>,
}

impl Parts {
    /// Part `seq` of `sum` of message `id`: the whole payload once every
    /// part is here.
    pub fn add(&mut self, id: &str, sum: usize, seq: usize, payload: Vec<u8>) -> Option<Vec<u8>> {
        if sum <= 1 {
            return Some(payload);
        }
        if seq >= sum || sum > 1024 {
            return None;
        }
        let now = Instant::now();
        self.open.retain(|_, (t, _)| now.duration_since(*t) < PARTS_KEPT);
        let (_, parts) = self.open.entry(id.to_string()).or_insert_with(|| (now, vec![None; sum]));
        if parts.len() != sum {
            return None;
        }
        parts[seq] = Some(payload);
        if parts.iter().any(Option::is_none) {
            return None;
        }
        let (_, parts) = self.open.remove(id).expect("just there");
        Some(parts.into_iter().flatten().flatten().collect())
    }
}

/// How the connection is to be kept, as Feishu says (seconds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientConfig {
    pub reconnect_interval: u64,
    pub reconnect_nonce: u64,
    pub ping_interval: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig { reconnect_interval: 120, reconnect_nonce: 30, ping_interval: 120 }
    }
}

impl ClientConfig {
    /// What a `ClientConfig` object says, over what this one has.
    pub fn update(&mut self, v: &Value) {
        let secs = |k: &str| v.get(k).and_then(Value::as_u64).filter(|s| *s > 0);
        if let Some(s) = secs("ReconnectInterval") {
            self.reconnect_interval = s;
        }
        if let Some(s) = v.get("ReconnectNonce").and_then(Value::as_u64) {
            self.reconnect_nonce = s;
        }
        if let Some(s) = secs("PingInterval") {
            self.ping_interval = s;
        }
    }
}

/// An answer of the open platform that is not `code` 0: what it said.
fn refused(v: &Value) -> Option<String> {
    let code = v.get("code").and_then(Value::as_i64)?;
    if code == 0 {
        return None;
    }
    let msg = v.get("msg").and_then(Value::as_str).unwrap_or("");
    Some(format!("Feishu refused ({code}): {msg}"))
}

fn json_of(body: &str) -> Result<Value, String> {
    serde_json::from_str(body.trim()).map_err(|_| format!("Feishu answered what is not JSON: {}", gist(body)))
}

fn gist(s: &str) -> String {
    let s = s.trim();
    let mut out: String = s.chars().take(120).collect();
    if s.chars().count() > 120 {
        out.push('…');
    }
    out
}

/// A request to the open platform: where, and its JSON body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub body: String,
}

/// The long connection's address is asked for with the app's id and secret.
pub fn endpoint_request(base: &str, app_id: &str, secret: &str) -> Request {
    Request {
        url: format!("{base}/callback/ws/endpoint"),
        body: json!({ "AppID": app_id, "AppSecret": secret }).to_string(),
    }
}

/// The connection's address and how to keep it.
pub fn parse_endpoint(body: &str) -> Result<(String, ClientConfig), String> {
    let v = json_of(body)?;
    if let Some(why) = refused(&v) {
        return Err(why);
    }
    let url = v["data"]["URL"].as_str().filter(|u| !u.is_empty()).ok_or("Feishu gave no address")?;
    let mut cfg = ClientConfig::default();
    cfg.update(&v["data"]["ClientConfig"]);
    Ok((url.to_string(), cfg))
}

/// The `service_id` in the connection's address: what its pings carry.
pub fn service_id(url: &str) -> i32 {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    query.split('&').find_map(|kv| kv.strip_prefix("service_id=")).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// The app's token for sending (two hours).
pub fn token_request(base: &str, app_id: &str, secret: &str) -> Request {
    Request {
        url: format!("{base}/open-apis/auth/v3/tenant_access_token/internal"),
        body: json!({ "app_id": app_id, "app_secret": secret }).to_string(),
    }
}

/// The token and how many seconds it holds.
pub fn parse_token(body: &str) -> Result<(String, u64), String> {
    let v = json_of(body)?;
    if let Some(why) = refused(&v) {
        return Err(why);
    }
    let token = v["tenant_access_token"].as_str().filter(|t| !t.is_empty()).ok_or("Feishu gave no token")?;
    Ok((token.to_string(), v["expire"].as_u64().unwrap_or(7200)))
}

/// The most of a message's text sent, in characters (Feishu takes 150 KB).
pub const TEXT_MAX: usize = 4000;

/// A text message's `content`: JSON in a string.
fn text_content(text: &str) -> String {
    let mut t: String = text.chars().take(TEXT_MAX).collect();
    if text.chars().count() > TEXT_MAX {
        t.push('…');
    }
    json!({ "text": t }).to_string()
}

/// A text to a person, by their open_id.
pub fn send_request(base: &str, open_id: &str, text: &str) -> Request {
    Request {
        url: format!("{base}/open-apis/im/v1/messages?receive_id_type=open_id"),
        body: json!({ "receive_id": open_id, "msg_type": "text", "content": text_content(text) }).to_string(),
    }
}

/// A text in answer to a message of theirs (shown under it).
pub fn reply_request(base: &str, message_id: &str, text: &str) -> Request {
    Request {
        url: format!("{base}/open-apis/im/v1/messages/{message_id}/reply"),
        body: json!({ "msg_type": "text", "content": text_content(text) }).to_string(),
    }
}

/// The id of the message just sent.
pub fn parse_sent(body: &str) -> Result<String, String> {
    let v = json_of(body)?;
    if let Some(why) = refused(&v) {
        return Err(why);
    }
    v["data"]["message_id"].as_str().map(String::from).ok_or_else(|| "Feishu gave no message id".into())
}

/// A message someone sent the bot (`im.message.receive_v1`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Incoming {
    pub event_id: String,
    pub message_id: String,
    /// `p2p` (to the bot alone) or `group`.
    pub chat_type: String,
    /// `text`, `post`, `image`, …
    pub message_type: String,
    /// The text, for a text message; `@` mentions taken out.
    pub text: String,
    /// Who sent it: their open_id, and `user` for a person.
    pub sender: String,
    pub sender_type: String,
    /// The message it answers, and the first of its thread.
    pub parent_id: String,
    pub root_id: String,
    /// When it was sent, in Unix milliseconds.
    pub create_ms: i64,
}

/// What an event's payload is, for keepane: a message, something else
/// (None), or what is wrong with it.
pub fn parse_event(payload: &[u8]) -> Result<Option<Incoming>, String> {
    let v: Value = serde_json::from_slice(payload).map_err(|_| "an event that is not JSON".to_string())?;
    if v.get("encrypt").is_some() {
        return Err("events come encrypted: clear the Encrypt Key in the app's event settings".into());
    }
    if v["header"]["event_type"].as_str() != Some("im.message.receive_v1") {
        return Ok(None);
    }
    let m = &v["event"]["message"];
    let s = |x: &Value| x.as_str().unwrap_or_default().to_string();
    let message_type = s(&m["message_type"]);
    let text = if message_type == "text" {
        let content: Value = serde_json::from_str(m["content"].as_str().unwrap_or("{}")).unwrap_or_default();
        strip_mentions(content["text"].as_str().unwrap_or_default())
    } else {
        String::new()
    };
    Ok(Some(Incoming {
        event_id: s(&v["header"]["event_id"]),
        message_id: s(&m["message_id"]),
        chat_type: s(&m["chat_type"]),
        message_type,
        text,
        sender: s(&v["event"]["sender"]["sender_id"]["open_id"]),
        sender_type: s(&v["event"]["sender"]["sender_type"]),
        parent_id: s(&m["parent_id"]),
        root_id: s(&m["root_id"]),
        create_ms: m["create_time"].as_str().and_then(|t| t.parse().ok()).unwrap_or(0),
    }))
}

/// `@_user_1` and the like (how Feishu writes a mention in a text) out.
fn strip_mentions(text: &str) -> String {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.starts_with("@_user_") && *w != "@_all").collect();
    words.join(" ").trim().to_string()
}

/// What a person sends the bot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatCmd {
    /// `/panes`: the panes, and which wait for the person.
    Panes,
    /// `/to <pane> <text>`: a message, as `send-message` sends one.
    To { pane: String, text: String },
    /// `/key <pane> <keys or text>`: pressed now (`typed`).
    Key { pane: String, keys: String },
    /// `/screen <pane>`: what the pane shows last.
    Screen { pane: String },
    /// `/help`.
    Help,
    /// Anything else: text with no command in front.
    Text(String),
}

/// A pane and the rest, after a command: the first word, then everything
/// after the space that ends it (line breaks kept).
fn pane_and_rest(rest: &str) -> (String, String) {
    let rest = rest.trim_start();
    match rest.find(char::is_whitespace) {
        Some(i) => (rest[..i].to_string(), rest[i..].trim().to_string()),
        None => (rest.to_string(), String::new()),
    }
}

/// What a chat message asks for.
pub fn parse_chat(text: &str) -> Result<ChatCmd, String> {
    let t = text.trim();
    let Some(cmd) = t.strip_prefix('/') else {
        return Ok(if t.eq_ignore_ascii_case("help") { ChatCmd::Help } else { ChatCmd::Text(t.to_string()) });
    };
    let (word, rest) = match cmd.find(char::is_whitespace) {
        Some(i) => (&cmd[..i], &cmd[i..]),
        None => (cmd, ""),
    };
    let (pane, more) = pane_and_rest(rest);
    let need = |what: &str| format!("/{word}: {what}; /help for the commands");
    match word.to_ascii_lowercase().as_str() {
        "panes" | "ls" if pane.is_empty() => Ok(ChatCmd::Panes),
        "help" | "?" | "h" => Ok(ChatCmd::Help),
        "to" if pane.is_empty() || more.is_empty() => Err(need("a pane, then the text (/to %3 run the tests)")),
        "to" => Ok(ChatCmd::To { pane, text: more }),
        "key" | "keys" if pane.is_empty() || more.is_empty() => Err(need("a pane, then the keys (/key %3 1)")),
        "key" | "keys" => Ok(ChatCmd::Key { pane, keys: more }),
        "screen" if pane.is_empty() || !more.is_empty() => Err(need("one pane (/screen %3)")),
        "screen" => Ok(ChatCmd::Screen { pane }),
        "panes" | "ls" => Err(need("takes nothing")),
        _ => Err(format!("/{word} is not a command; /help for the commands")),
    }
}

/// The commands, as `/help` answers.
pub const HELP: &str = "keepane commands:
/panes  the panes, and which wait for you
/to <pane> <text>  a message for the pane (an agent gets it when it is free)
/key <pane> <keys>  keys pressed now: 1, y, n, esc, enter, up, down, tab; other text is typed, then Enter
/screen <pane>  what the pane shows last
/help  this
Reply to a notification to answer that pane: to one that waits for you, a key or text goes in at once; to others, as a message.
A pane is %3, %name, or session:window.pane.";

/// A word as a key `send-keys` takes: a key's name in any case (`esc`,
/// `Enter`, `up`), or one character as it is. None for a word that is text.
pub fn key_name(word: &str) -> Option<String> {
    let named = match word.to_ascii_lowercase().as_str() {
        "esc" | "escape" => "Escape",
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "btab" | "shift-tab" | "s-tab" => "BTab",
        "space" => "Space",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "bs" | "bspace" | "backspace" => "BSpace",
        "ctrl-c" | "c-c" | "^c" => "C-c",
        "ctrl-d" | "c-d" | "^d" => "C-d",
        _ => {
            let mut cs = word.chars();
            return match (cs.next(), cs.next()) {
                (Some(c), None) if !c.is_whitespace() => Some(word.to_string()),
                _ => None,
            };
        }
    };
    Some(named.into())
}

/// What goes into a pane for keys a person sent: each word pressed, when
/// every word is a key; else the whole text typed, then Enter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    Keys(Vec<String>),
    Line(String),
}

pub fn typed(text: &str) -> Typed {
    let t = text.trim();
    let keys: Option<Vec<String>> = t.split_whitespace().map(key_name).collect();
    match keys {
        Some(k) if !k.is_empty() && !t.contains('\n') => Typed::Keys(k),
        _ => Typed::Line(t.to_string()),
    }
}

/// `keepane feishu [status|allow|remove] …` as the server's command:
/// `feishu-status`, `feishu-allow …`, `feishu-remove …`.
pub fn command_args(rest: &[String]) -> Vec<String> {
    let (first, more) = match rest.split_first() {
        Some((w, more)) if !w.starts_with('-') => (w.as_str(), more),
        _ => ("status", rest),
    };
    let mut args = vec![format!("feishu-{first}")];
    args.extend(more.iter().cloned());
    args
}

/// An open_id as Feishu writes one: `ou_` and letters and digits.
pub fn valid_open_id(id: &str) -> bool {
    id.len() > 3 && id.len() <= 64 && id.starts_with("ou_") && id[3..].bytes().all(|b| b.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame reads back as it was written, every field; a ping as
    /// Feishu's SDK builds one (proto2, its four required fields set, so
    /// written even when 0: the bytes below) reads as it says, and keepane
    /// writes the same bytes; an unknown field is passed over; a cut frame
    /// is refused.
    #[test]
    fn frames_round_trip_and_read_feishus() {
        let f = Frame {
            seq_id: 7,
            log_id: 1 << 40,
            service: -3,
            method: METHOD_DATA,
            headers: vec![("type".into(), "event".into()), ("message_id".into(), "m1".into())],
            payload_encoding: "".into(),
            payload_type: "json".into(),
            payload: b"{\"a\":1}".to_vec(),
            log_id_new: "x".into(),
        };
        assert_eq!(Frame::decode(&f.encode()).unwrap(), f);
        // SeqID 0, LogID 0, service 21, method 0, one header type=ping.
        let ping = [
            0x08, 0x00, 0x10, 0x00, 0x18, 0x15, 0x20, 0x00, 0x2a, 0x0c, 0x0a, 0x04, b't', b'y', b'p', b'e', 0x12, 0x04,
            b'p', b'i', b'n', b'g',
        ];
        assert_eq!(Frame::decode(&ping).unwrap(), Frame::ping(21));
        assert_eq!(Frame::ping(21).encode(), ping, "the same bytes as the SDK");
        let mut more = ping.to_vec();
        more.extend([0x58, 0x05]); // field 11, a varint
        assert_eq!(Frame::decode(&more).unwrap(), Frame::ping(21));
        assert!(Frame::decode(&ping[..ping.len() - 1]).is_err());
        assert!(Frame::decode(&[0xff; 11]).is_err());
        let ack = f.ack(12);
        assert_eq!(ack.header("biz_rt"), Some("12"));
        assert_eq!(ack.header("message_id"), Some("m1"));
        assert_eq!(ack.seq_id, 7);
        let answer: Value = serde_json::from_slice(&ack.payload).unwrap();
        assert_eq!(answer["code"], 200);
    }

    /// Parts come together in any order, only once all are here; one
    /// alone is whole; a part out of range is dropped.
    #[test]
    fn parts_come_together() {
        let mut p = Parts::default();
        assert_eq!(p.add("a", 1, 0, b"x".to_vec()), Some(b"x".to_vec()));
        assert_eq!(p.add("b", 3, 2, b"3".to_vec()), None);
        assert_eq!(p.add("b", 3, 0, b"1".to_vec()), None);
        assert_eq!(p.add("b", 3, 9, b"9".to_vec()), None);
        assert_eq!(p.add("b", 3, 1, b"2".to_vec()), Some(b"123".to_vec()));
        assert!(p.open.is_empty());
        assert_eq!(p.add("c", 2, 0, b"1".to_vec()), None);
        assert_eq!(p.add("c", 3, 1, b"2".to_vec()), None, "a different sum is not the same message");
    }

    /// The endpoint's answer as Feishu documents it: the address, the
    /// service in it, and the intervals; a refusal says Feishu's words.
    #[test]
    fn the_endpoint_is_read() {
        let body = r#"{"code":0,"msg":"ok","data":{"URL":"wss://msg-frontier.feishu.cn/ws/v2?fpid=493&aid=552564&device_id=7&access_key=k&service_id=33554678&ticket=t","ClientConfig":{"ReconnectCount":-1,"ReconnectInterval":120,"ReconnectNonce":30,"PingInterval":90}}}"#;
        let (url, cfg) = parse_endpoint(body).unwrap();
        assert!(url.starts_with("wss://msg-frontier.feishu.cn/ws/v2?"));
        assert_eq!(service_id(&url), 33554678);
        assert_eq!(cfg, ClientConfig { reconnect_interval: 120, reconnect_nonce: 30, ping_interval: 90 });
        let no = parse_endpoint(r#"{"code":10003,"msg":"invalid param"}"#).unwrap_err();
        assert_eq!(no, "Feishu refused (10003): invalid param");
        assert!(parse_endpoint("<html>").unwrap_err().contains("not JSON"));
        assert_eq!(service_id("wss://x/ws"), 0);
        let mut c = ClientConfig::default();
        c.update(&json!({"PingInterval": 0, "ReconnectNonce": 0}));
        assert_eq!(c.ping_interval, 120, "0 is no interval");
        assert_eq!(c.reconnect_nonce, 0);
    }

    /// The requests carry what Feishu documents, and the answers read.
    #[test]
    fn requests_and_answers() {
        assert_eq!(base_url(""), FEISHU);
        assert_eq!(base_url("lark"), LARK);
        assert_eq!(base_url("http://127.0.0.1:9/"), "http://127.0.0.1:9");
        assert!(valid_domain("feishu") && valid_domain("http://127.0.0.1:9") && !valid_domain("ftp://x"));
        let e = endpoint_request(FEISHU, "cli_1", "s\"x");
        assert_eq!(e.url, "https://open.feishu.cn/callback/ws/endpoint");
        let v: Value = serde_json::from_str(&e.body).unwrap();
        assert_eq!((v["AppID"].as_str(), v["AppSecret"].as_str()), (Some("cli_1"), Some("s\"x")));
        let t = token_request(LARK, "cli_1", "s");
        assert_eq!(t.url, "https://open.larksuite.com/open-apis/auth/v3/tenant_access_token/internal");
        assert_eq!(
            parse_token(r#"{"code":0,"msg":"ok","tenant_access_token":"t-1","expire":7140}"#).unwrap(),
            ("t-1".into(), 7140)
        );
        assert!(
            parse_token(r#"{"code":10014,"msg":"app secret invalid"}"#).unwrap_err().contains("app secret invalid")
        );
        let s = send_request(FEISHU, "ou_1", "hi \"you\"");
        assert!(s.url.ends_with("/open-apis/im/v1/messages?receive_id_type=open_id"));
        let v: Value = serde_json::from_str(&s.body).unwrap();
        assert_eq!(v["receive_id"], "ou_1");
        assert_eq!(v["msg_type"], "text");
        let content: Value = serde_json::from_str(v["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["text"], "hi \"you\"");
        let long = "字".repeat(TEXT_MAX + 5);
        let v: Value = serde_json::from_str(&reply_request(FEISHU, "om_1", &long).body).unwrap();
        let content: Value = serde_json::from_str(v["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["text"].as_str().unwrap().chars().count(), TEXT_MAX + 1, "cut, with …");
        assert!(reply_request(FEISHU, "om_1", "x").url.ends_with("/open-apis/im/v1/messages/om_1/reply"));
        assert_eq!(parse_sent(r#"{"code":0,"data":{"message_id":"om_9"}}"#).unwrap(), "om_9");
        assert!(parse_sent(r#"{"code":230002,"msg":"bot not in chat"}"#).is_err());
    }

    /// A message event as Feishu documents it: who, where, what it
    /// answers, the text without mentions; other events are not messages;
    /// an encrypted one says how to stop that.
    #[test]
    fn a_message_event_is_read() {
        let payload = json!({
            "schema": "2.0",
            "header": {"event_id": "ev1", "event_type": "im.message.receive_v1", "create_time": "1700000000000"},
            "event": {
                "sender": {"sender_id": {"open_id": "ou_abc", "user_id": "u"}, "sender_type": "user"},
                "message": {
                    "message_id": "om_2", "parent_id": "om_1", "root_id": "om_0", "create_time": "1700000000123",
                    "chat_id": "oc_1", "chat_type": "p2p", "message_type": "text",
                    "content": "{\"text\":\"@_user_1 /to %3 run it\"}"
                }
            }
        });
        let m = parse_event(payload.to_string().as_bytes()).unwrap().unwrap();
        assert_eq!(m.text, "/to %3 run it");
        assert_eq!((m.sender.as_str(), m.chat_type.as_str()), ("ou_abc", "p2p"));
        assert_eq!((m.parent_id.as_str(), m.root_id.as_str(), m.create_ms), ("om_1", "om_0", 1_700_000_000_123));
        assert_eq!(m.event_id, "ev1");
        let other = json!({"schema": "2.0", "header": {"event_type": "im.chat.member.bot.added_v1"}, "event": {}});
        assert_eq!(parse_event(other.to_string().as_bytes()).unwrap(), None);
        assert!(parse_event(br#"{"encrypt":"abc"}"#).unwrap_err().contains("Encrypt Key"));
        assert!(parse_event(b"nope").is_err());
        let mut image = payload.clone();
        image["event"]["message"]["message_type"] = json!("image");
        image["event"]["message"]["content"] = json!("{\"image_key\":\"k\"}");
        let m = parse_event(image.to_string().as_bytes()).unwrap().unwrap();
        assert_eq!((m.message_type.as_str(), m.text.as_str()), ("image", ""));
    }

    /// The commands a chat sends, and text that is none.
    #[test]
    fn chat_commands() {
        assert_eq!(parse_chat("/panes"), Ok(ChatCmd::Panes));
        assert_eq!(parse_chat(" /LS "), Ok(ChatCmd::Panes));
        assert_eq!(parse_chat("help"), Ok(ChatCmd::Help));
        assert_eq!(parse_chat("/help"), Ok(ChatCmd::Help));
        assert_eq!(
            parse_chat("/to %3 fix it\nthen test"),
            Ok(ChatCmd::To { pane: "%3".into(), text: "fix it\nthen test".into() })
        );
        assert_eq!(parse_chat("/key %build esc"), Ok(ChatCmd::Key { pane: "%build".into(), keys: "esc".into() }));
        assert_eq!(parse_chat("/screen work:0.1"), Ok(ChatCmd::Screen { pane: "work:0.1".into() }));
        assert_eq!(parse_chat("just words"), Ok(ChatCmd::Text("just words".into())));
        assert!(parse_chat("/to %3").unwrap_err().contains("then the text"));
        assert!(parse_chat("/key").is_err());
        assert!(parse_chat("/screen %1 %2").is_err());
        assert!(parse_chat("/panes x").is_err());
        assert!(parse_chat("/rm -rf").unwrap_err().contains("not a command"));
    }

    /// Keys: a key's name in any case, one character as it is; text that
    /// is not all keys goes in as a line.
    #[test]
    fn keys_and_lines() {
        assert_eq!(key_name("ESC").as_deref(), Some("Escape"));
        assert_eq!(key_name("1").as_deref(), Some("1"));
        assert_eq!(key_name("是").as_deref(), Some("是"));
        assert_eq!(key_name("yes"), None);
        assert_eq!(typed("1"), Typed::Keys(vec!["1".into()]));
        assert_eq!(typed(" down down enter "), Typed::Keys(vec!["Down".into(), "Down".into(), "Enter".into()]));
        assert_eq!(typed("yes please"), Typed::Line("yes please".into()));
        assert_eq!(typed("a\nb"), Typed::Line("a\nb".into()));
        assert!(valid_open_id("ou_7d8a6e6df7621556ce0d21922b676706ccs"));
        assert!(!valid_open_id("ou_") && !valid_open_id("on_abc") && !valid_open_id("ou_a b"));
        let args = |w: &[&str]| command_args(&w.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(args(&[]), ["feishu-status"]);
        assert_eq!(args(&["allow", "ou_1", "--shell"]), ["feishu-allow", "ou_1", "--shell"]);
        assert_eq!(args(&["remove", "ou_1"]), ["feishu-remove", "ou_1"]);
        assert_eq!(args(&["--shell"]), ["feishu-status", "--shell"], "a flag alone: status, which refuses it");
    }
}
