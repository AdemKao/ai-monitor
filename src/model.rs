use std::ops::AddAssign;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub messages: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

impl Usage {
    pub fn active_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.reasoning_tokens
    }

    pub fn all_tokens(&self) -> u64 {
        self.active_tokens() + self.cache_read_tokens + self.cache_write_tokens
    }
}

impl AddAssign<&Usage> for Usage {
    fn add_assign(&mut self, other: &Usage) {
        self.messages += other.messages;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.cost_usd += other.cost_usd;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BreakdownUsage {
    pub calls: u64,
    pub sessions: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

impl BreakdownUsage {
    pub fn active_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.reasoning_tokens
    }

    pub fn all_tokens(&self) -> u64 {
        self.active_tokens() + self.cache_read_tokens + self.cache_write_tokens
    }

    pub fn add_usage(&mut self, usage: &Usage) {
        self.calls = self.calls.saturating_add(usage.messages);
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(usage.reasoning_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(usage.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(usage.cache_write_tokens);
        self.cost_usd += usage.cost_usd;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageRow {
    pub day: String,
    pub provider: String,
    pub model: String,
    #[serde(flatten)]
    pub usage: Usage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageReport {
    pub source: String,
    pub start_day: String,
    pub end_day: String,
    pub scope: String,
    pub rows: Vec<UsageRow>,
}

#[cfg(test)]
mod tests {
    use super::Usage;

    #[test]
    fn calculates_token_totals() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 20,
            reasoning_tokens: 30,
            cache_read_tokens: 40,
            cache_write_tokens: 50,
            ..Usage::default()
        };

        assert_eq!(usage.active_tokens(), 60);
        assert_eq!(usage.all_tokens(), 150);
    }
}
