//! transaction history storage

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// direction of transaction
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TxDirection {
    Sent,
    Received,
}

/// transaction status
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TxStatus {
    Pending,
    Confirmed,
    Failed,
}

/// a transaction record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxRecord {
    pub txid: String,
    pub direction: TxDirection,
    pub amount: u64,         // zatoshis
    pub address: String,     // recipient or sender
    pub memo: Option<String>,
    pub timestamp: u64,      // unix timestamp
    pub height: Option<u32>, // block height when confirmed
    pub status: TxStatus,
    pub contact_name: Option<String>, // resolved contact name
}

impl TxRecord {
    pub fn new_sent(txid: &str, address: &str, amount: u64, memo: Option<String>) -> Self {
        Self {
            txid: txid.to_string(),
            direction: TxDirection::Sent,
            amount,
            address: address.to_string(),
            memo,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            height: None,
            status: TxStatus::Pending,
            contact_name: None,
        }
    }

    pub fn new_received(txid: &str, address: &str, amount: u64, memo: Option<String>, height: u32) -> Self {
        Self {
            txid: txid.to_string(),
            direction: TxDirection::Received,
            amount,
            address: address.to_string(),
            memo,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            height: Some(height),
            status: TxStatus::Confirmed,
            contact_name: None,
        }
    }

    pub fn confirm(&mut self, height: u32) {
        self.status = TxStatus::Confirmed;
        self.height = Some(height);
    }

    pub fn fail(&mut self) {
        self.status = TxStatus::Failed;
    }
}

/// transaction history storage (ring buffer, keeps last N)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TxHistory {
    records: VecDeque<TxRecord>,
    max_records: usize,
}

impl TxHistory {
    pub fn new() -> Self {
        Self {
            records: VecDeque::new(),
            max_records: 500,
        }
    }

    pub fn add(&mut self, record: TxRecord) {
        // check for duplicate txid
        if self.records.iter().any(|r| r.txid == record.txid) {
            return;
        }
        self.records.push_front(record);
        while self.records.len() > self.max_records {
            self.records.pop_back();
        }
    }

    pub fn update_status(&mut self, txid: &str, status: TxStatus, height: Option<u32>) {
        if let Some(record) = self.records.iter_mut().find(|r| r.txid == txid) {
            record.status = status;
            if let Some(h) = height {
                record.height = Some(h);
            }
        }
    }

    pub fn get(&self, txid: &str) -> Option<&TxRecord> {
        self.records.iter().find(|r| r.txid == txid)
    }

    /// list all transactions (newest first)
    pub fn list(&self) -> impl Iterator<Item = &TxRecord> {
        self.records.iter()
    }

    /// list sent transactions
    pub fn sent(&self) -> impl Iterator<Item = &TxRecord> {
        self.records.iter().filter(|r| r.direction == TxDirection::Sent)
    }

    /// list received transactions
    pub fn received(&self) -> impl Iterator<Item = &TxRecord> {
        self.records.iter().filter(|r| r.direction == TxDirection::Received)
    }

    /// pending transactions
    pub fn pending(&self) -> impl Iterator<Item = &TxRecord> {
        self.records.iter().filter(|r| r.status == TxStatus::Pending)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
