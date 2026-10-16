//! Runtime-independent, allocation-free async client over `embedded_io_async`.
//! Supply a connector that targets one origin and returns a fresh authenticated
//! transport for each call. DNS, TLS, networking and deadlines belong to the
//! application, so no executor or network stack is selected by the library.
use crate::{Authentication, Body, Call, Response, session::Session};
use embedded_io_async::{Read, Write};

/// Connect to one configured origin. A fresh stream is obtained for each call.
/// TLS connectors must authenticate the configured host before returning it.
#[allow(async_fn_in_trait)]
pub trait Connector {
    type Error;
    type Connection<'a>: Read + Write
    where
        Self: 'a;
    async fn connect(&mut self) -> Result<Self::Connection<'_>, Self::Error>;
}

#[derive(Debug)]
pub enum Error<E> {
    Connect(E),
    Core(crate::Error),
}
impl<E> From<crate::Error> for Error<E> {
    fn from(e: crate::Error) -> Self {
        Self::Core(e)
    }
}

/// Owns routing and a connector, borrows reusable header storage. Response bytes
/// stay in the caller's output buffer. Cancellation drops the in-flight stream;
/// the next call always reconnects, without replaying the cancelled request.
pub struct Client<'a, C> {
    core: crate::Client<'a>,
    connector: C,
    headers: &'a mut [u8],
}
impl<'a, C: Connector> Client<'a, C> {
    pub fn new(core: crate::Client<'a>, connector: C, headers: &'a mut [u8]) -> Self {
        Self {
            core,
            connector,
            headers,
        }
    }
    pub async fn execute<'b>(
        &mut self,
        call: &Call,
        body: Option<&[u8]>,
        output: &'b mut [u8],
    ) -> Result<Response<'b>, Error<C::Error>> {
        self.execute_body(call, body.map(Body::Json), output).await
    }
    pub async fn execute_body<'b>(
        &mut self,
        call: &Call,
        body: Option<Body<'_>>,
        output: &'b mut [u8],
    ) -> Result<Response<'b>, Error<C::Error>> {
        crate::client::validate_request(call, body)?;
        let mut stream = self.connector.connect().await.map_err(Error::Connect)?;
        self.core
            .execute_with(&mut stream, call, body, self.headers, output, |_, _| Ok(()))
            .await
            .map_err(Error::Core)
    }
    /// The origin's jar supplies outgoing authentication and receives atomic
    /// response-header updates, including when reading the body fails.
    pub async fn execute_with_session<'b, const K: usize, const T: usize>(
        &mut self,
        call: &Call,
        body: Option<Body<'_>>,
        output: &'b mut [u8],
        session: &mut Session<K, T>,
    ) -> Result<Response<'b>, Error<C::Error>> {
        crate::client::validate_request(call, body)?;
        let snapshot = session.clone();
        let core = crate::Client::with_authentication(
            self.core.host,
            self.core.base,
            snapshot.authentication().unwrap_or(Authentication::None),
        )
        .map_err(|e| Error::Core(e.into()))?;
        let mut stream = self.connector.connect().await.map_err(Error::Connect)?;
        core.execute_with_session(&mut stream, call, body, self.headers, output, session)
            .await
            .map_err(Error::Core)
    }
    pub fn into_connector(self) -> C {
        self.connector
    }
}
