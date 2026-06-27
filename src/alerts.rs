use std::collections::VecDeque;
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct Alert {
    pub ts: f64,
    pub level: String,
    pub kind: String,
    pub message: String,
    pub target: String,
}

pub struct AlertStore {
    alerts: VecDeque<Alert>,
    max_size: usize,
}

impl AlertStore {
    pub fn new(max_size: usize) -> Self {
        Self { alerts: VecDeque::with_capacity(max_size), max_size }
    }
    pub fn push(&mut self, alert: Alert) {
        if self.alerts.len() >= self.max_size { self.alerts.pop_front(); }
        self.alerts.push_back(alert);
    }
    pub fn recent(&self, n: usize) -> Vec<&Alert> {
        self.alerts.iter().rev().take(n).collect()
    }
}
