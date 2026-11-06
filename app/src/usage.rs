//! The usage meter, reduced to what the tray shows: the session (5-hour) window, the weekly window,
//! and per-model weekly windows. Read from `limits[]` when the answer has it (2.1.3xx), else from the
//! older `five_hour` / `seven_day` fields.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Window {
    pub label: String,
    /// 0..=100 (can exceed 100 when over the limit).
    pub percent: f64,
    /// Unix ms when the window resets.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Usage {
    pub session: Option<Window>,
    pub week: Option<Window>,
    #[serde(default)]
    pub scoped: Vec<Window>,
    /// Unix ms the reading was taken.
    pub fetched_at: i64,
    /// "api" (our poll) or "claude" (Claude Code's own cached reading).
    pub source: String,
}

impl Usage {
    /// This reading as of `now`: a window whose reset time has passed has started over at 0.
    pub fn as_of(&self, now: i64) -> Usage {
        let roll = |w: &Window| {
            let mut w = w.clone();
            if w.resets_at.is_some_and(|r| r <= now) {
                w.percent = 0.0;
                w.resets_at = None;
            }
            w
        };
        Usage {
            session: self.session.as_ref().map(roll),
            week: self.week.as_ref().map(roll),
            scoped: self.scoped.iter().map(roll).collect(),
            fetched_at: self.fetched_at,
            source: self.source.clone(),
        }
    }
}

fn time_ms(v: Option<&Value>) -> Option<i64> {
    let s = v?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.timestamp_millis())
}

fn window(label: &str, percent: Option<f64>, resets: Option<&Value>) -> Option<Window> {
    Some(Window {
        label: label.to_owned(),
        percent: percent?,
        resets_at: time_ms(resets),
    })
}

/// Reads a usage answer (`GET /api/oauth/usage`, or the `utilization` Claude Code caches).
pub fn parse(v: &Value, fetched_at: i64, source: &str) -> Usage {
    let mut usage = Usage {
        fetched_at,
        source: source.to_owned(),
        ..Usage::default()
    };
    if let Some(limits) = v.get("limits").and_then(Value::as_array) {
        for limit in limits {
            let kind = limit.get("kind").and_then(Value::as_str).unwrap_or_default();
            let percent = limit.get("percent").and_then(Value::as_f64);
            let resets = limit.get("resets_at");
            match kind {
                "session" => usage.session = window("Session", percent, resets),
                "weekly_all" => usage.week = window("Week", percent, resets),
                "weekly_scoped" => {
                    let scope = limit.get("scope");
                    let name = scope
                        .and_then(|s| s.pointer("/model/display_name"))
                        .or_else(|| scope.and_then(|s| s.get("surface")))
                        .and_then(Value::as_str)
                        .unwrap_or("Scoped");
                    if let Some(w) = window(&format!("{name} week"), percent, resets) {
                        usage.scoped.push(w);
                    }
                }
                _ => {}
            }
        }
    }
    let util = |key: &str| {
        let w = v.get(key).filter(|w| w.is_object())?;
        Some((w.get("utilization").and_then(Value::as_f64), w.get("resets_at")))
    };
    if usage.session.is_none()
        && let Some((p, r)) = util("five_hour")
    {
        usage.session = window("Session", p, r);
    }
    if usage.week.is_none()
        && let Some((p, r)) = util("seven_day")
    {
        usage.week = window("Week", p, r);
    }
    if usage.scoped.is_empty() {
        for (key, name) in [("seven_day_opus", "Opus week"), ("seven_day_sonnet", "Sonnet week")] {
            if let Some((p, r)) = util(key)
                && let Some(w) = window(name, p, r)
            {
                usage.scoped.push(w);
            }
        }
    }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn limits_are_preferred_and_scoped_windows_named() {
        let v = json!({
            "five_hour": {"utilization": 1, "resets_at": "2026-11-04T09:40:00+00:00"},
            "limits": [
                {"kind": "session", "percent": 27, "resets_at": "2026-11-04T09:40:00.187700+00:00"},
                {"kind": "weekly_all", "percent": 54, "resets_at": "2026-11-07T08:00:00+00:00"},
                {"kind": "weekly_scoped", "percent": 3, "resets_at": "2026-11-07T08:00:00+00:00",
                 "scope": {"model": {"display_name": "Fable"}}}
            ]
        });
        let u = parse(&v, 5, "api");
        assert_eq!(u.session.as_ref().unwrap().percent, 27.0);
        assert_eq!(u.week.as_ref().unwrap().percent, 54.0);
        assert_eq!(u.scoped[0].label, "Fable week");
    }

    #[test]
    fn older_shape_and_rollover() {
        let v = json!({"five_hour": {"utilization": 80, "resets_at": "2020-01-01T00:00:00Z"},
                       "seven_day": {"utilization": 20, "resets_at": "2099-01-01T00:00:00Z"}});
        let u = parse(&v, 0, "claude");
        assert_eq!(u.session.as_ref().unwrap().percent, 80.0);
        let now = u.as_of(chrono::Utc::now().timestamp_millis());
        assert_eq!(now.session.unwrap().percent, 0.0, "a passed reset starts the window over");
        assert_eq!(now.week.unwrap().percent, 20.0);
    }
}
