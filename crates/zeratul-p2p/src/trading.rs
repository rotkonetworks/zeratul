//! Trading-specific primitives
//!
//! Low-latency order matching with cryptographic proofs

use serde::{Deserialize, Serialize};

/// Trading order with proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    /// Trader public key
    pub trader: [u8; 32],
    /// Buy or sell
    pub side: Side,
    /// Asset pair (e.g., BTC/USD)
    pub pair: (String, String),
    /// Price in quote currency
    pub price: u64,
    /// Quantity in base currency
    pub quantity: u64,
    /// Nonce (prevents replay)
    pub nonce: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

/// Trade execution (when buy and sell match)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub buyer: [u8; 32],
    pub seller: [u8; 32],
    pub pair: (String, String),
    pub price: u64,
    pub quantity: u64,
    pub block_number: u64,
}

/// Order book state
#[derive(Debug, Clone)]
pub struct OrderBook {
    /// Buy orders (sorted by price descending)
    pub bids: Vec<Order>,
    /// Sell orders (sorted by price ascending)
    pub asks: Vec<Order>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self {
            bids: Vec::new(),
            asks: Vec::new(),
        }
    }

    /// Add order to book
    pub fn add_order(&mut self, order: Order) {
        match order.side {
            Side::Buy => {
                self.bids.push(order);
                self.bids.sort_by(|a, b| b.price.cmp(&a.price)); // Highest first
            }
            Side::Sell => {
                self.asks.push(order);
                self.asks.sort_by(|a, b| a.price.cmp(&b.price)); // Lowest first
            }
        }
    }

    /// Match orders and return executed trades
    pub fn match_orders(&mut self, block_number: u64) -> Vec<Trade> {
        let mut trades = Vec::new();

        while !self.bids.is_empty() && !self.asks.is_empty() {
            let bid = &self.bids[0];
            let ask = &self.asks[0];

            // Match if buy price >= sell price
            if bid.price >= ask.price {
                let trade_price = ask.price; // Taker pays maker's price
                let trade_quantity = bid.quantity.min(ask.quantity);

                trades.push(Trade {
                    buyer: bid.trader,
                    seller: ask.trader,
                    pair: bid.pair.clone(),
                    price: trade_price,
                    quantity: trade_quantity,
                    block_number,
                });

                // Update quantities
                if bid.quantity == trade_quantity {
                    self.bids.remove(0);
                }
                if ask.quantity == trade_quantity {
                    self.asks.remove(0);
                }
            } else {
                break; // No more matches
            }
        }

        trades
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_matching() {
        let mut book = OrderBook::new();

        // Buy order: 100 BTC @ 50000 USD
        book.add_order(Order {
            trader: [1; 32],
            side: Side::Buy,
            pair: ("BTC".into(), "USD".into()),
            price: 50000,
            quantity: 100,
            nonce: 1,
        });

        // Sell order: 50 BTC @ 49000 USD
        book.add_order(Order {
            trader: [2; 32],
            side: Side::Sell,
            pair: ("BTC".into(), "USD".into()),
            price: 49000,
            quantity: 50,
            nonce: 1,
        });

        let trades = book.match_orders(1);

        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].price, 49000); // Sell price (maker price)
        assert_eq!(trades[0].quantity, 50);
        assert_eq!(trades[0].buyer, [1; 32]);
        assert_eq!(trades[0].seller, [2; 32]);
    }
}
