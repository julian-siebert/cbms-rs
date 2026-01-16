use cbms::transport::{StreamTransport, Transport};
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Database, MySqlPool, Pool, query};

use crate::adapter::SQLAdapter;

mod adapter;
mod stub;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPayload {
    query: String,
    params: Vec<ciborium::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResultPayload {
    rows_returned: u64,
}

async fn test() {
    let pool = MySqlPool::connect("url").await.unwrap();
    let transport = StreamTransport::stdio();
    let adapter = SQLAdapter::new(transport, pool);
}
