//! A way for server function calls to reach a server in the same process as requests.

use std::{future::Future, pin::Pin, sync::Arc};

use axum::body::Body;
use dioxus_core::Runtime;
use dioxus_fullstack_core::RequestError;
use http::{Request, Response};

use crate::reqwest_error_to_request_error;

type Send = Arc<
    dyn Fn(Request<Body>) -> Pin<Box<dyn Future<Output = Response<Body>> + std::marker::Send>>
        + std::marker::Send
        + Sync,
>;

/// Where a component tree's server function calls go instead of the network.
///
/// A test that runs the client app and its server in one process provides one as a context. The
/// app's server calls then reach the server as the requests a browser would send, through its
/// router, middleware and session layer, in the server build as well as the client. Without one,
/// the client build sends them over the network and the server build calls them directly.
#[derive(Clone)]
pub struct ServerFnTransport {
    send: Send,
}

impl ServerFnTransport {
    /// A transport that answers each request with `send`.
    pub fn new<F, Fut>(send: F) -> Self
    where
        F: Fn(Request<Body>) -> Fut + std::marker::Send + Sync + 'static,
        Fut: Future<Output = Response<Body>> + std::marker::Send + 'static,
    {
        Self {
            send: Arc::new(move |request| Box::pin(send(request))),
        }
    }

    /// The transport the running component's tree provides, if any. None outside a component.
    pub fn current() -> Option<Self> {
        let runtime = Runtime::try_current()?;
        let scope = runtime.try_current_scope_id()?;
        runtime.in_scope(scope, dioxus_core::try_consume_context::<Self>)
    }

    pub(crate) async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, RequestError> {
        let request: Request<reqwest::Body> = request
            .build()
            .and_then(TryInto::try_into)
            .map_err(reqwest_error_to_request_error)?;
        let response = (self.send)(request.map(Body::new)).await;
        Ok(response
            .map(|body| reqwest::Body::wrap_stream(body.into_data_stream()))
            .into())
    }
}

/// Sends `request` through the component tree's transport, or over the network without one.
pub(crate) async fn send(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, RequestError> {
    match ServerFnTransport::current() {
        Some(transport) => transport.send(request).await,
        None => request.send().await.map_err(reqwest_error_to_request_error),
    }
}
