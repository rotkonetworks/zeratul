//! Strategy serialization — export/import for WASM bot.
//!
//! The solved strategy is a mapping:
//!   InfoSetKey → [action probabilities]
//!
//! Serialized as a compact binary file that the browser loads.
//! The bot reads this at play time to make decisions.

use std::collections::HashMap;
use super::abstraction::InfoSetKey;
use super::solver::InfoNode;

/// export strategy table to binary format
pub fn export_strategy(nodes: &HashMap<InfoSetKey, InfoNode>) -> Vec<u8> {
    let mut buf = Vec::new();

    // header: number of nodes
    let count = nodes.len() as u32;
    buf.extend_from_slice(&count.to_le_bytes());

    for (key, node) in nodes {
        // key bytes
        let key_bytes = key.to_bytes();
        buf.push(key_bytes.len() as u8);
        buf.extend_from_slice(&key_bytes);

        // average strategy (this is the Nash equilibrium output)
        let avg = node.average_strategy();
        buf.push(avg.len() as u8);
        for &p in &avg {
            // store as u16 fixed point (0..65535 = 0.0..1.0)
            let fixed = (p * 65535.0).round().min(65535.0) as u16;
            buf.extend_from_slice(&fixed.to_le_bytes());
        }
    }

    buf
}

/// import strategy table from binary format
pub fn import_strategy(data: &[u8]) -> HashMap<Vec<u8>, Vec<f64>> {
    let mut map = HashMap::new();
    let mut pos = 0;

    if data.len() < 4 { return map; }
    let count = u32::from_le_bytes(data[pos..pos+4].try_into().unwrap()) as usize;
    pos += 4;

    for _ in 0..count {
        if pos >= data.len() { break; }

        let key_len = data[pos] as usize;
        pos += 1;
        if pos + key_len > data.len() { break; }
        let key = data[pos..pos+key_len].to_vec();
        pos += key_len;

        if pos >= data.len() { break; }
        let num_actions = data[pos] as usize;
        pos += 1;

        let mut strategy = Vec::with_capacity(num_actions);
        for _ in 0..num_actions {
            if pos + 2 > data.len() { break; }
            let fixed = u16::from_le_bytes(data[pos..pos+2].try_into().unwrap());
            strategy.push(fixed as f64 / 65535.0);
            pos += 2;
        }

        map.insert(key, strategy);
    }

    map
}

/// export from generic HashMap<Vec<u8>, InfoNode> (used by multi_solver)
pub fn export_strategy_from_nodes(nodes: &HashMap<Vec<u8>, InfoNode>) -> Vec<u8> {
    let mut buf = Vec::new();
    let count = nodes.len() as u32;
    buf.extend_from_slice(&count.to_le_bytes());

    for (key_bytes, node) in nodes {
        buf.push(key_bytes.len() as u8);
        buf.extend_from_slice(key_bytes);

        let avg = node.average_strategy();
        buf.push(avg.len() as u8);
        for &p in &avg {
            let fixed = (p * 65535.0).round().min(65535.0) as u16;
            buf.extend_from_slice(&fixed.to_le_bytes());
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::solver::InfoNode;

    #[test]
    fn test_roundtrip() {
        let mut nodes = HashMap::new();
        let key = InfoSetKey { hand_bucket: 42, history: vec![1, 2], street: 0 };
        let mut node = InfoNode::new(3);
        node.strategy_sum = vec![100.0, 200.0, 50.0]; // ~29%, 57%, 14%
        nodes.insert(key.clone(), node);

        let data = export_strategy(&nodes);
        let imported = import_strategy(&data);

        let key_bytes = key.to_bytes();
        assert!(imported.contains_key(&key_bytes));
        let strat = &imported[&key_bytes];
        assert_eq!(strat.len(), 3);
        // check approximate values (fixed point precision)
        assert!((strat[0] - 0.2857).abs() < 0.01);
        assert!((strat[1] - 0.5714).abs() < 0.01);
        assert!((strat[2] - 0.1429).abs() < 0.01);
    }
}
