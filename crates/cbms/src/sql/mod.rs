use sqlx::{Database, Pool};

use crate::transport::Transport;

pub struct SQLAdapter<T, DB>
where
    T: Transport,
    DB: Database,
{
    transport: T,
    pool: Pool<DB>,
}

impl<T, DB> SQLAdapter<T, DB>
where
    T: Transport,
    DB: Database,
{
    pub fn new(transport: T, pool: Pool<DB>) -> Self {
        Self { transport, pool }
    }
}
