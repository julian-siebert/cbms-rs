use cbms::{Error, transport::Transport};

use crate::{QueryPayload, QueryResultPayload};

pub struct Stub<T>
where
    T: Transport,
{
    transport: T,
}

impl<T> Stub<T>
where
    T: Transport,
{
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn query(payload: QueryPayload) -> Result<QueryResultPayload, Error> {
        Ok(QueryResultPayload { rows_returned: 2 })
    }
}
