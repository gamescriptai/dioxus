use std::ops::Deref;

use crate::ServerFnError;
use axum::extract::FromRequestParts;
use http::request::Parts;
use serde::de::DeserializeOwned;

/// `Config::new` requires this value; five preserves the limit used by `serde_qs::from_str`.
const SERDE_QS_DEFAULT_MAX_DEPTH: usize = 5;

/// An extractor that deserializes query parameters into the given type `T`.
///
/// This uses `serde_qs` under the hood to support complex query parameter structures.
#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ServerFnError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let inner: T = deserialize_query(parts.uri.query().unwrap_or_default())
            .map_err(|e| ServerFnError::Deserialization(e.to_string()))?;
        Ok(Self(inner))
    }
}

/// Preserve strict parsing for ordinary queries, then let `serde_qs` tolerate structural brackets
/// percent-encoded by clients such as URLSession without rewriting the query or its values.
fn deserialize_query<T: DeserializeOwned>(query: &str) -> Result<T, serde_qs::Error> {
    serde_qs::from_str(query).or_else(|_| {
        serde_qs::Config::new(SERDE_QS_DEFAULT_MAX_DEPTH, false).deserialize_str(query)
    })
}

impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct RangeQuery {
        range: std::ops::Range<u32>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct ScalarQuery {
        value: String,
    }

    fn extract<T: DeserializeOwned>(query: &str) -> Result<T, ServerFnError> {
        let request = Request::builder()
            .uri(format!("/?{query}"))
            .body(())
            .unwrap();
        let (mut parts, _) = request.into_parts();

        futures::executor::block_on(Query::<T>::from_request_parts(&mut parts, &()))
            .map(|query| query.0)
    }

    #[test]
    fn encoded_nested_struct_keys_match_raw_bracket_keys() {
        let raw = extract::<RangeQuery>("range[start]=10&range[end]=20").unwrap();
        let encoded = extract::<RangeQuery>("range%5Bstart%5D=10&range%5Bend%5D=20").unwrap();

        assert_eq!(encoded, raw);
    }

    #[test]
    fn encoded_brackets_in_values_remain_value_data() {
        let query = extract::<ScalarQuery>("value=literal%5B0%5D").unwrap();

        assert_eq!(query.value, "literal[0]");
    }
}
