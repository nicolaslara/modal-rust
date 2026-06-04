//! The [`Function`] handle: real in-process `.local()` plus the locked
//! `.remote()`/`.spawn()`/`.map()` async surface.
//!
//! `.local()` dispatches through the FROZEN [`Registry`](crate::Registry) exactly
//! as the runner would, minus the subprocess: `serde_json::to_vec(&input)` →
//! `(HandlerFn)(&bytes)` → `serde_json::from_slice(&out)`. So
//! `add(AddInput{a:40,b:2})` yields `AddOutput{sum:42}`, identical to the CLI
//! runner.

use crate::{Error, HandlerFn, Result};

/// A handle to a single registered entrypoint of an [`App`](crate::App).
///
/// Obtained via [`App::function`](crate::App::function). The handler is resolved
/// at construction (cheap, `Copy`); an unknown name is reported as
/// [`Error::UnknownEntrypoint`] when an operation is actually invoked.
pub struct Function<'a> {
    pub(crate) app: &'a crate::App,
    /// Owned: `App::function` takes `&str`.
    pub(crate) name: String,
    /// `Some` if the entrypoint was found in the registry, `None` if unknown.
    pub(crate) handler: Option<HandlerFn>,
}

impl Function<'_> {
    /// Run the registered handler IN-PROCESS via the FROZEN [`Registry`](crate::Registry):
    /// serialize `input` to JSON, invoke the [`HandlerFn`], deserialize the JSON
    /// output to `Out`. Zero Modal, zero network. Mirrors Modal Python's
    /// `Function.local()` = `raw_f(*args)`.
    ///
    /// The double JSON round-trip is intentional and correct: input → JSON →
    /// handler's own `codec::decode` → `In`; handler's `codec::encode` → JSON →
    /// `from_slice` → `Out`. This is identical to running the runner without a
    /// subprocess.
    ///
    /// # Errors
    /// - [`Error::UnknownEntrypoint`] if the name is not registered.
    /// - [`Error::Encode`] if `input` fails to serialize to JSON.
    /// - [`Error::Runner`] wrapping any of the five frozen runtime failure kinds
    ///   (handler-side decode of `In`, function body `Err`, encode of `Out`, panic).
    /// - [`Error::Decode`] if the handler's JSON output does not match `Out`.
    pub fn local<In, Out>(&self, input: In) -> Result<Out>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
    {
        let handler = self.handler.ok_or_else(|| self.unknown())?;
        let bytes = serde_json::to_vec(&input).map_err(Error::Encode)?;
        let out = handler(&bytes).map_err(Error::Runner)?;
        serde_json::from_slice(&out).map_err(Error::Decode)
    }

    /// Run the function body REMOTELY on Modal (the RUN path), returning the typed
    /// output with the SAME semantics as [`Function::local`].
    ///
    /// Requires a connected App ([`App::connect`](crate::App::connect)) — otherwise
    /// [`Error::NotConnected`]. On first call the App ensures the wrapper function
    /// exists (uploads the source crate as a mount, builds the run image, creates +
    /// publishes the FILE-mode function); subsequent calls reuse the memoized
    /// `function_id`. The user's Rust crate is `cargo build`-ed IN THE FUNCTION
    /// BODY at invoke time (the run boundary), then `modal_runner` execs the
    /// registered handler and emits the same JSON envelope `.local()` produces.
    ///
    /// # Errors
    /// - [`Error::NotConnected`] if the App was not connected.
    /// - [`Error::Encode`] if `input` fails to serialize to JSON.
    /// - [`Error::Sdk`] for any control-plane / upload / build / invoke failure
    ///   (including a remote `cargo build` failure, surfaced with its traceback).
    /// - [`Error::Runner`] wrapping the frozen five-kind taxonomy (identical to
    ///   `.local()`) when the handler itself reports a structured failure.
    /// - [`Error::Decode`] if the envelope / output does not match `Out`.
    pub async fn remote<In, Out>(&self, input: In) -> Result<Out>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
    {
        let input_json = serde_json::to_string(&input).map_err(Error::Encode)?;
        let envelope = self.app.remote_invoke(&self.name, input_json).await?;
        crate::remote::parse_envelope::<Out>(&envelope)
    }

    /// Fire-and-forget spawn returning a [`FunctionCall`] handle. NOT YET
    /// IMPLEMENTED: returns [`Error::NotImplemented`] (a later milestone; use
    /// [`Function::remote`] for a single typed remote call today).
    #[allow(clippy::unused_async)]
    pub async fn spawn<In>(&self, input: In) -> Result<FunctionCall>
    where
        In: serde::Serialize,
    {
        let _ = input;
        Err(Error::not_implemented("Function::spawn"))
    }

    /// Fan-out over many inputs. NOT YET IMPLEMENTED: returns
    /// [`Error::NotImplemented`] (a later milestone; use [`Function::remote`] per
    /// input today).
    #[allow(clippy::unused_async)]
    pub async fn map<In, Out, I>(&self, inputs: I) -> Result<Vec<Out>>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
        I: IntoIterator<Item = In>,
    {
        let _ = inputs;
        Err(Error::not_implemented("Function::map"))
    }

    /// Build the [`Error::UnknownEntrypoint`] for this handle, listing the App's
    /// known names.
    fn unknown(&self) -> Error {
        Error::UnknownEntrypoint {
            name: self.name.clone(),
            known: self.app.known_names(),
        }
    }
}

/// Handle returned by [`Function::spawn`]. Locks the `spawn().get()` shape without
/// depending on the SDK's internal call-handle type yet.
pub struct FunctionCall {
    _private: (),
}

impl FunctionCall {
    /// Await the spawned call's result. NOT YET IMPLEMENTED: returns
    /// [`Error::NotImplemented`] (see [`Function::remote`]).
    #[allow(clippy::unused_async)]
    pub async fn get<Out>(&self, timeout: Option<std::time::Duration>) -> Result<Out>
    where
        Out: serde::de::DeserializeOwned,
    {
        let _ = timeout;
        Err(Error::not_implemented("FunctionCall::get"))
    }
}
