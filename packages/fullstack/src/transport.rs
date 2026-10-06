//! A way for server function calls to reach a server in the same process as requests.

use std::{future::Future, pin::Pin, sync::Arc};

use axum::body::Body;
use dioxus_core::Runtime;
use dioxus_fullstack_core::RequestError;
use http::{Request, Response};

use crate::reqwest_error_to_request_error;

/// The handler a transport calls with each request. Boxed so that any async handler fits one
/// concrete `ServerFnTransport` type.
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

    /// The transport the running component's tree provides, if any.
    ///
    /// Returns `None` outside a component, such as in a server handler or a background task,
    /// because there is no tree to look in. Each step checks instead of panicking, since server
    /// functions are called from those places too.
    pub fn current() -> Option<Self> {
        let runtime = Runtime::try_current()?;
        let scope = runtime.try_current_scope_id()?;
        // Look up from the running component, so a provider anywhere above it counts.
        runtime.in_scope(scope, dioxus_core::try_consume_context::<Self>)
    }

    /// Sends `request` to this transport and returns its reply as if it came over the network.
    ///
    /// The client code builds requests with reqwest, while a server router takes `http` types
    /// with an axum body. This converts the request on the way in and the response on the way
    /// out, so the code above gets an ordinary `reqwest::Response`.
    pub(crate) async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, RequestError> {
        // Finish the reqwest builder and turn it into a plain `http::Request`.
        let request: Request<reqwest::Body> = request
            .build()
            .and_then(TryInto::try_into)
            .map_err(reqwest_error_to_request_error)?;
        // Hand the request to the handler with its body as an axum body.
        let response = (self.send)(request.map(Body::new)).await;

        // Stream the reply body back as a reqwest body, without buffering it.
        Ok(response
            .map(|body| reqwest::Body::wrap_stream(body.into_data_stream()))
            .into())
    }
}

/// Sends `request` through the component tree's transport, or over the network without one.
///
/// Every native send in `ClientRequest` goes through here, so this `match` is where a call picks
/// its path. Production code never provides a transport and always takes the network branch, as
/// it did before transports existed.
pub(crate) async fn send(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, RequestError> {
    match ServerFnTransport::current() {
        Some(transport) => transport.send(request).await,
        None => request.send().await.map_err(reqwest_error_to_request_error),
    }
}
