use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum TradeType {
    #[default] // Mark Buy as the default variant
    Buy,
    Sell,
    // might wanna add removing and adding stocks
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trade {
    pub trade_type: TradeType,
    pub item: String,
    pub amount: i32,
    pub amount_currency: f64,
    pub user_uuid: String, // maybe also name or even User
    pub timestamp: DateTime<Utc>,
}

impl Trade {
    // saving single

    // loading single

    // saving all

    // loading all
}
