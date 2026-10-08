//! What a model's tokens cost, for the agents' `cost` (`agents`): LiteLLM's
//! public price list (every provider's models, kept up to date by its
//! project), fetched with `curl` on a thread of its own once a day (an hour
//! later after a failed try), and only while `agent-cost` is on and a pane
//! runs an agent. What it said
//! is kept in the data directory (`prices.json`, only the few numbers used),
//! so a restarted server, or one offline, still knows. `KEEPANE_PRICE_LIST`
//! names a list of one's own in the same form instead: nothing is fetched.

use super::agents::Tokens;
use super::{Event, Server};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// A failed fetch (offline) is tried again this much later.
const RETRY: Duration = Duration::from_secs(60 * 60);

/// One model's prices, dollars a token, and its context window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// Reading the cache, writing it (kept five minutes), writing it kept an
    /// hour; None where the list says nothing (the input's price is taken).
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub cache_write_1h: Option<f64>,
    pub window: Option<u64>,
}

impl Price {
    pub fn cost(&self, t: &Tokens) -> f64 {
        let write = self.cache_write.unwrap_or(self.input);
        let short = t.cache_write.saturating_sub(t.cache_write_1h);
        t.input as f64 * self.input
            + t.output as f64 * self.output
            + t.cache_read as f64 * self.cache_read.unwrap_or(self.input)
            + short as f64 * write
            + t.cache_write_1h as f64 * self.cache_write_1h.unwrap_or(write)
    }
}

/// The price list: models by name, and when it was fetched (Unix seconds).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prices {
    models: HashMap<String, Price>,
    pub fetched: u64,
}

fn price_of(v: &Value) -> Option<Price> {
    let f = |k: &str| v[k].as_f64();
    Some(Price {
        input: f("input_cost_per_token")?,
        output: f("output_cost_per_token")?,
        cache_read: f("cache_read_input_token_cost"),
        cache_write: f("cache_creation_input_token_cost"),
        cache_write_1h: f("cache_creation_input_token_cost_above_1hr"),
        window: v["max_input_tokens"].as_u64(),
    })
}

impl Prices {
    /// LiteLLM's list: only the models under their own names (`gpt-5.5`, not
    /// `azure/gpt-5.5`): the names the agents write, at their maker's price.
    pub fn from_litellm(text: &str, fetched: u64) -> Option<Prices> {
        let v: Value = serde_json::from_str(text).ok()?;
        let models: HashMap<String, Price> = v
            .as_object()?
            .iter()
            .filter(|(k, _)| !k.contains('/'))
            .filter_map(|(k, e)| Some((k.to_ascii_lowercase(), price_of(e)?)))
            .collect();
        (!models.is_empty()).then_some(Prices { models, fetched })
    }

    /// As kept: `{"fetched": secs, "models": {name: [input, output,
    /// cache_read, cache_write, cache_write_1h, window]}}`.
    pub fn to_cache(&self) -> String {
        let models: serde_json::Map<String, Value> = self
            .models
            .iter()
            .map(|(k, p)| {
                let row =
                    serde_json::json!([p.input, p.output, p.cache_read, p.cache_write, p.cache_write_1h, p.window]);
                (k.clone(), row)
            })
            .collect();
        serde_json::json!({ "fetched": self.fetched, "models": models }).to_string()
    }

    pub fn from_cache(text: &str) -> Option<Prices> {
        let v: Value = serde_json::from_str(text).ok()?;
        let models: HashMap<String, Price> = v["models"]
            .as_object()?
            .iter()
            .filter_map(|(k, r)| {
                Some((
                    k.clone(),
                    Price {
                        input: r[0].as_f64()?,
                        output: r[1].as_f64()?,
                        cache_read: r[2].as_f64(),
                        cache_write: r[3].as_f64(),
                        cache_write_1h: r[4].as_f64(),
                        window: r[5].as_u64(),
                    },
                ))
            })
            .collect();
        (!models.is_empty()).then(|| Prices { models, fetched: v["fetched"].as_u64().unwrap_or(0) })
    }

    /// The price of the model an agent names: by its name; without a
    /// provider before it (`openai/`) or a mark after it (`[1m]`); without
    /// a date at its end; else the longest listed name it begins with,
    /// followed by `-` (`gpt-5.5-codex` is `gpt-5.5`'s).
    pub fn of(&self, model: &str) -> Option<&Price> {
        let m = model.trim().to_ascii_lowercase();
        let m = m.rsplit('/').next().unwrap_or(&m);
        let m = m.split('[').next().unwrap_or(m).trim();
        if m.is_empty() {
            return None;
        }
        if let Some(p) = self.models.get(m) {
            return Some(p);
        }
        let undated = match m.rsplit_once('-') {
            Some((head, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => head,
            _ => m,
        };
        if let Some(p) = self.models.get(undated) {
            return Some(p);
        }
        self.models
            .iter()
            .filter(|(k, _)| {
                undated.len() > k.len() && undated.starts_with(k.as_str()) && undated.as_bytes()[k.len()] == b'-'
            })
            .max_by_key(|(k, _)| k.len())
            .map(|(_, p)| p)
    }
}

#[derive(Default)]
pub(super) struct PriceList {
    pub prices: Option<Arc<Prices>>,
    due: Option<SystemTime>,
    asking: bool,
    /// Where the list is kept; None: the data directory's `prices.json`.
    cache: Option<std::path::PathBuf>,
}

impl PriceList {
    fn cache(&self) -> std::path::PathBuf {
        self.cache.clone().unwrap_or_else(|| crate::logger::log_dir().join("prices.json"))
    }
}

impl Server {
    /// Every tick: when cost is on and an agent runs, read the kept list (the
    /// first time), and fetch a fresh one when it is a day old.
    pub(super) fn prices_tick(&mut self) {
        if !self.opts.agent_cost || self.agents.stats.is_empty() || self.prices.asking {
            return;
        }
        // A list of one's own (LiteLLM's form; a machine kept offline, the
        // tests): read once, nothing fetched.
        if let Some(own) = crate::legacy::var_os("KEEPANE_PRICE_LIST") {
            if self.prices.due.is_none() {
                let now = SystemTime::now();
                let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let list = std::fs::read_to_string(own).ok().and_then(|t| Prices::from_litellm(&t, secs));
                self.prices.prices = list.map(Arc::new);
                self.prices.due = Some(now + Duration::from_secs(100 * 365 * 24 * 60 * 60));
            }
            return;
        }
        if self.prices.prices.is_none() && self.prices.due.is_none() {
            let kept = std::fs::read_to_string(self.prices.cache()).ok().and_then(|t| Prices::from_cache(&t));
            if let Some(p) = kept {
                self.prices.due = Some(UNIX_EPOCH + Duration::from_secs(p.fetched) + EVERY);
                self.prices.prices = Some(Arc::new(p));
            }
        }
        if self.prices.due.is_some_and(|d| SystemTime::now() < d) {
            return;
        }
        self.prices.asking = true;
        super::ask_on_thread(&self.events, || fetch().map(Box::new), Event::Prices);
    }

    pub(super) fn prices_fetched(&mut self, prices: Option<Box<Prices>>) {
        self.prices.asking = false;
        let now = SystemTime::now();
        let Some(p) = prices else {
            self.prices.due = Some(now + RETRY);
            return;
        };
        self.prices.due = Some(now + EVERY);
        let _ = std::fs::write(self.prices.cache(), p.to_cache());
        self.prices.prices = Some(Arc::new(*p));
    }
}

/// The list, fetched now; None when it could not be.
fn fetch() -> Option<Prices> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "-m", "30", "-H", "User-Agent: keepane", URL])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Prices::from_litellm(&String::from_utf8_lossy(&out.stdout), now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{
        "sample_spec": {"input_cost_per_token": "the cost"},
        "claude-opus-5-5": {"input_cost_per_token": 4e-06, "output_cost_per_token": 2e-05,
            "cache_read_input_token_cost": 2e-07, "cache_creation_input_token_cost": 5e-06,
            "cache_creation_input_token_cost_above_1hr": 8e-06, "max_input_tokens": 1000000},
        "anthropic.claude-opus-5-5": {"input_cost_per_token": 9.0, "output_cost_per_token": 9.0},
        "gpt-5.5": {"input_cost_per_token": 5e-06, "output_cost_per_token": 3e-05,
            "cache_read_input_token_cost": 5e-07, "max_input_tokens": 1050000},
        "gpt-5": {"input_cost_per_token": 1.25e-06, "output_cost_per_token": 1e-05},
        "azure/gpt-5.5": {"input_cost_per_token": 1.0, "output_cost_per_token": 1.0},
        "dall-e-3": {"output_cost_per_image": 0.04}
    }"#;

    #[test]
    fn a_model_is_found_by_the_name_its_agent_writes() {
        let p = Prices::from_litellm(LIST, 7).unwrap();
        assert_eq!(p.of("claude-opus-5-5").unwrap().input, 4e-06);
        assert_eq!(p.of("Claude-Opus-5-5").unwrap().input, 4e-06, "any case");
        assert_eq!(p.of("anthropic/claude-opus-5-5").unwrap().input, 4e-06, "a provider before it");
        assert_eq!(p.of("claude-opus-5-5[1m]").unwrap().input, 4e-06, "a mark after it");
        assert_eq!(p.of("claude-opus-5-5-20260901").unwrap().input, 4e-06, "a date at its end");
        assert_eq!(p.of("gpt-5.5-codex").unwrap().input, 5e-06, "its family's, the longest");
        assert_eq!(p.of("gpt-5.5").unwrap().window, Some(1050000));
        assert!(p.of("gpt-5.55").is_none(), "begins with gpt-5.5 but is not of it");
        assert!(p.of("").is_none() && p.of("llama-x").is_none());
        assert!(p.of("dall-e-3").is_none(), "priced by the image, not the token: left out");
        assert!(p.of("sample_spec").is_none(), "no numbers: left out");
    }

    #[test]
    fn tokens_are_priced_each_at_its_own_rate() {
        let p = Prices::from_litellm(LIST, 7).unwrap();
        let t = Tokens { input: 1_000, output: 2_000, cache_read: 10_000, cache_write: 3_000, cache_write_1h: 1_000 };
        let opus = p.of("claude-opus-5-5").unwrap().cost(&t);
        let want = 1_000.0 * 4e-06 + 2_000.0 * 2e-05 + 10_000.0 * 2e-07 + 2_000.0 * 5e-06 + 1_000.0 * 8e-06;
        assert!((opus - want).abs() < 1e-12, "{opus} {want}");
        // No cache prices listed: the input's.
        let gpt5 = p.of("gpt-5").unwrap().cost(&t);
        let want = (1_000.0 + 10_000.0 + 3_000.0) * 1.25e-06 + 2_000.0 * 1e-05;
        assert!((gpt5 - want).abs() < 1e-12, "{gpt5} {want}");
    }

    #[test]
    fn the_kept_list_reads_back_as_it_was() {
        let p = Prices::from_litellm(LIST, 1_790_000_000).unwrap();
        let back = Prices::from_cache(&p.to_cache()).unwrap();
        assert_eq!(back, p);
        assert_eq!(Prices::from_cache("garbage"), None);
        assert_eq!(Prices::from_cache(r#"{"fetched":1,"models":{}}"#), None, "an empty list is as none");
        assert_eq!(Prices::from_litellm("[]", 0), None);
    }

    /// Nothing is fetched while cost is off or no agent runs; once both
    /// hold, the kept list is read and, a day old, a fresh one asked for.
    /// A failure keeps what was known. (The fetch itself is not run here: a
    /// test must not reach the internet; the list kept is fresh.)
    #[test]
    fn it_is_fetched_only_while_cost_is_on_and_an_agent_runs() {
        let dir = std::env::temp_dir().join(format!("keepane-prices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("prices.json");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        std::fs::write(&cache, Prices::from_litellm(LIST, now).unwrap().to_cache()).unwrap();
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let mut s = Server::new(pane_tx, "t".into(), events);
        s.prices.cache = Some(cache.clone());
        s.prices_tick();
        assert!(s.prices.prices.is_none() && !s.prices.asking, "no agent: nothing read, nothing asked");
        s.agents.stats.insert(1, super::super::agents::Stats::default());
        s.opts.agent_cost = false;
        s.prices_tick();
        assert!(s.prices.prices.is_none() && !s.prices.asking, "cost off");
        s.opts.agent_cost = true;
        s.prices_tick();
        assert!(s.prices.prices.is_some(), "the kept list is read");
        assert!(!s.prices.asking, "kept today: not asked again");
        let known = s.prices.prices.clone();
        s.prices_fetched(None);
        assert_eq!(s.prices.prices, known, "a failed fetch keeps what was known");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
